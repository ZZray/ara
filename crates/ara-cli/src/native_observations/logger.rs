//! One existing coarse-family helper for original synchronous logger WIP.
//! Controlled effect ports prove graph/order behavior, not real OS logging,
//! Promise/context execution or complete logger/product acceptance.
use ara_cli::native_realm::{
    NativeGlobals,
    logger::{LoggerFile, LoggerHost, NativeLoggerBindings, NativeLoggerDomain},
};
use ara_rpc::{
    WireString,
    value_heap::{
        CapabilityId, CompiledFunctionForm, CompiledFunctionSpec, PropertyDescriptor, RuntimeReply, RuntimeRequest,
        ValueError, ValueHandle, ValueHeap, ValueRuntime, ValueSnapshot,
    },
};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, Weak},
};
type Result<T> = std::result::Result<T, ValueError>;
const LOGGER_PORT: CapabilityId = CapabilityId(0x4152415f4c4f4747);

#[derive(Default)]
struct Probe {
    env: HashMap<String, WireString>,
    clock: VecDeque<f64>,
    clock_calls: usize,
    files: Vec<WireString>,
    fd1: Vec<WireString>,
    fd2: Vec<WireString>,
    console: Vec<WireString>,
    order: Vec<&'static str>,
}
struct ProbeHost {
    probe: Arc<Mutex<Probe>>,
}
struct ProbeFile {
    probe: Arc<Mutex<Probe>>,
}
impl LoggerFile for ProbeFile {
    fn write(&self, line: &ValueHandle, runtime: &dyn ValueRuntime) -> Result<()> {
        self.probe.lock().map_err(|_| ValueError::PoisonedHeap)?.order.push("begin_write");
        let text = line.heap().to_string(line, runtime)?;
        let mut probe = self.probe.lock().map_err(|_| ValueError::PoisonedHeap)?;
        probe.order.push("append");
        probe.files.push(text);
        Ok(())
    }
    fn close(&self) -> Result<()> {
        self.probe.lock().map_err(|_| ValueError::PoisonedHeap)?.order.push("close");
        Ok(())
    }
}
impl LoggerHost for ProbeHost {
    fn environment(&self, name: &str) -> Result<Option<WireString>> {
        Ok(self.probe.lock().map_err(|_| ValueError::PoisonedHeap)?.env.get(name).cloned())
    }
    fn cwd(&self, _: &Arc<ValueHeap>, _: &dyn ValueRuntime) -> Result<WireString> {
        Ok("/fixture".into())
    }
    fn pid(&self) -> Result<f64> {
        Ok(123.0)
    }
    fn eol(&self) -> Result<WireString> {
        Ok("\n".into())
    }
    fn performance_now(&self) -> Result<f64> {
        let mut probe = self.probe.lock().map_err(|_| ValueError::PoisonedHeap)?;
        probe.clock_calls += 1;
        probe.clock.pop_front().ok_or(ValueError::UnsupportedOperation("Controlled logger clock exhausted"))
    }
    fn default_logs_directory(&self) -> Result<WireString> {
        Ok("/explicit-temp".into())
    }
    fn make_file(&self, _: &WireString, _: &Arc<ValueHeap>, _: &dyn ValueRuntime) -> Result<Arc<dyn LoggerFile>> {
        self.probe.lock().map_err(|_| ValueError::PoisonedHeap)?.order.push("make_file");
        Ok(Arc::new(ProbeFile { probe: self.probe.clone() }))
    }
    fn write_sync(&self, fd: u32, text: &WireString, _: &Arc<ValueHeap>, _: &dyn ValueRuntime) -> Result<usize> {
        let mut probe = self.probe.lock().map_err(|_| ValueError::PoisonedHeap)?;
        match fd {
            1 => probe.fd1.push(text.clone()),
            2 => probe.fd2.push(text.clone()),
            _ => return Err(ValueError::InvalidNode),
        };
        Ok(String::from_utf16_lossy(text.units()).len())
    }
    fn console_error(&self, text: &WireString, _: &Arc<ValueHeap>, _: &dyn ValueRuntime) -> Result<()> {
        self.probe.lock().map_err(|_| ValueError::PoisonedHeap)?.console.push(text.clone());
        Ok(())
    }
}
struct ProbeRuntime<'a> {
    parent: &'a dyn ValueRuntime,
    domain: Weak<NativeLoggerDomain>,
}
impl ValueRuntime for ProbeRuntime<'_> {
    fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply> {
        match request {
            RuntimeRequest::Call { capability, receiver, arguments } => {
                self.resolve_call(capability, receiver, arguments, self)
            }
            request => self.parent.resolve(request),
        }
    }
    fn resolve_call(
        &self,
        capability: CapabilityId,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> Result<RuntimeReply> {
        if capability == LOGGER_PORT {
            let domain = self.domain.upgrade().ok_or(ValueError::ExpiredCapability(LOGGER_PORT))?;
            return domain
                .resolve(&RuntimeRequest::Call { capability, receiver, arguments }, effective)
                .ok_or(ValueError::MissingCapability(LOGGER_PORT))?;
        }
        self.parent.resolve_call(capability, receiver, arguments, effective)
    }
    fn resolve_local_call(
        &self,
        token: u64,
        receiver: &ValueHandle,
        args: &[ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> Result<RuntimeReply> {
        self.parent.resolve_local_call(token, receiver, args, effective)
    }
}
static SINK: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Controlled original logger coarse event-identity recorder",
    name: "sink",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 1,
    body: |heap, captures, _, args, runtime| {
        heap.array_push(&captures[0], &[args.first().cloned().unwrap_or(heap.undefined()?)], runtime)?;
        heap.undefined()
    },
};
static JSON_OVERRIDE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Controlled original logger coarse global JSON override",
    name: "stringify",
    length: 3,
    form: CompiledFunctionForm::Builtin,
    capture_count: 1,
    body: |heap, captures, _, args, runtime| {
        heap.array_push(&captures[0], &[args.first().cloned().unwrap_or(heap.undefined()?)], runtime)?;
        heap.string("overridden".into())
    },
};
fn call(
    heap: &Arc<ValueHeap>,
    module: &ValueHandle,
    name: &str,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    heap.call(&heap.get(module, &name.into(), runtime)?, &heap.undefined()?, args, runtime)
}
fn object(heap: &Arc<ValueHeap>, fields: &[(&str, ValueHandle)], runtime: &dyn ValueRuntime) -> Result<ValueHandle> {
    let object = heap.object()?;
    for (key, value) in fields {
        heap.define_own_data(&object, (*key).into(), value.clone(), runtime)?;
    }
    Ok(object)
}
fn mode(probe: &Arc<Mutex<Probe>>, key: &str, value: &str) -> Result<()> {
    probe.lock().map_err(|_| ValueError::PoisonedHeap)?.env.insert(key.into(), value.into());
    Ok(())
}

pub struct LoggerSyncObservation {
    pub exports_exercised: usize,
    pub console_only_json_calls: usize,
    pub timing_output: WireString,
}

/// Root attaches this helper to the existing coarse family after same-realm
/// Date/JSON + real context preparation. No fallback/no-op ALS is created here.
pub fn observe_logger_sync(
    heap: &Arc<ValueHeap>,
    globals: &NativeGlobals,
    context: &ValueHandle,
    original_runtime: &dyn ValueRuntime,
) -> Result<LoggerSyncObservation> {
    let probe = Arc::new(Mutex::new(Probe { clock: [10.0, 100.0, 110.0].into_iter().collect(), ..Probe::default() }));
    let domain = Arc::new(NativeLoggerDomain::new(heap.heap_token(), Arc::new(ProbeHost { probe: probe.clone() })));
    let runtime = ProbeRuntime { parent: original_runtime, domain: Arc::downgrade(&domain) };
    let bindings = NativeLoggerBindings::prepare_sync(heap, globals, context, &runtime)?;
    let module = &bindings.sync_module;
    assert_eq!(heap.own_keys(module, &runtime)?.len(), 14);
    assert!(matches!(heap.snapshot(&heap.get(module, &"time".into(), &runtime)?)?, ValueSnapshot::Undefined));
    // Preparation is lazy and produces no Host output/clock/file effects.
    {
        let p = probe.lock().map_err(|_| ValueError::PoisonedHeap)?;
        assert!(p.order.is_empty());
        assert_eq!(p.clock_calls, 0);
        assert!(p.console.is_empty());
    }
    call(
        heap,
        module,
        "setTransports",
        &[object(heap, &[("file", heap.boolean(false)?), ("console", heap.boolean(false)?)], &runtime)?],
        &runtime,
    )?;
    let events = heap.array(vec![])?;
    let sink = heap.fresh_compiled_function(&SINK, &[events.clone()])?;
    let dispose = call(heap, module, "registerLogSink", &[sink.clone()], &runtime)?;
    call(heap, module, "registerLogSink", &[sink], &runtime)?;
    let context_value = object(heap, &[("marker", heap.string("original-context".into())?)], &runtime)?;
    let message = heap.string("original-message".into())?;
    for name in ["error", "warn", "info", "debug"] {
        assert!(matches!(
            heap.snapshot(&call(heap, module, name, &[message.clone(), context_value.clone()], &runtime)?)?,
            ValueSnapshot::Undefined
        ));
    }
    assert_eq!(heap.array_length(&events, &runtime)?, 4);
    for i in 0..4 {
        let event = heap.get(&events, &WireString::from(i.to_string()).into(), &runtime)?;
        assert!(heap.get(&event, &"message".into(), &runtime)?.same_identity(&message));
        assert!(heap.get(&event, &"context".into(), &runtime)?.same_identity(&context_value));
    }
    heap.call(&dispose, &heap.undefined()?, &[], &runtime)?;
    heap.call(&dispose, &heap.undefined()?, &[], &runtime)?;
    call(heap, module, "warn", &[message.clone()], &runtime)?;
    assert_eq!(heap.array_length(&events, &runtime)?, 4);

    let json_records = heap.array(vec![])?;
    let original_stringify = heap.get(&globals.json, &"stringify".into(), &runtime)?;
    heap.assign(
        &globals.json,
        "stringify".into(),
        heap.fresh_compiled_function(&JSON_OVERRIDE, &[json_records.clone()])?,
        &runtime,
    )?;
    call(
        heap,
        module,
        "setTransports",
        &[object(heap, &[("file", heap.boolean(false)?), ("console", heap.boolean(true)?)], &runtime)?],
        &runtime,
    )?;
    call(heap, module, "info", &[message.clone()], &runtime)?;
    assert_eq!(heap.array_length(&json_records, &runtime)?, 2);
    assert_eq!(probe.lock().map_err(|_| ValueError::PoisonedHeap)?.fd1, vec![WireString::from("overridden\n")]);
    call(
        heap,
        module,
        "setTransports",
        &[object(heap, &[("file", heap.boolean(true)?), ("console", heap.boolean(false)?)], &runtime)?],
        &runtime,
    )?;
    call(heap, module, "info", &[message], &runtime)?;
    assert_eq!(probe.lock().map_err(|_| ValueError::PoisonedHeap)?.order, ["make_file", "begin_write", "append"]);
    heap.assign(&globals.json, "stringify".into(), original_stringify, &runtime)?;
    call(heap, module, "setTransports", &[object(heap, &[("file", heap.boolean(false)?)], &runtime)?], &runtime)?;
    assert_eq!(probe.lock().map_err(|_| ValueError::PoisonedHeap)?.order.last(), Some(&"close"));

    mode(&probe, "PI_TIMING", "longer, x:+full;\tend")?;
    assert!(heap.truthy(&call(heap, module, "timingModeIncludes", &[heap.string("x".into())?], &runtime)?)?);
    assert!(heap.truthy(&call(heap, module, "shouldExitAfterTimings", &[], &runtime)?)?);
    mode(&probe, "PI_DEBUG_STARTUP", "1")?;
    call(heap, module, "startupMarker", &[heap.string("probe".into())?], &runtime)?;
    assert_eq!(probe.lock().map_err(|_| ValueError::PoisonedHeap)?.fd2, vec![WireString::from("[startup] probe\n")]);
    call(heap, module, "printTimings", &[], &runtime)?;
    assert_eq!(
        probe.lock().map_err(|_| ValueError::PoisonedHeap)?.console,
        vec![WireString::from("\n--- Startup Timings ---\n(no markers)\n")]
    );
    call(heap, module, "startTiming", &[], &runtime)?;
    call(heap, module, "startTiming", &[], &runtime)?;
    assert_eq!(probe.lock().map_err(|_| ValueError::PoisonedHeap)?.clock_calls, 1);
    let old_buffer = heap.call(&bindings.module_load_buffer, &heap.undefined()?, &[], &runtime)?;
    let event = object(
        heap,
        &[
            ("path", heap.string("/fixture/a.ts".into())?),
            ("start", heap.number(5.0)?),
            ("durationMs", heap.number(30.0)?),
            ("bodyMs", heap.number(10.0)?),
            ("imports", heap.array(vec![heap.string("/fixture/b.ts".into())?])?),
        ],
        &runtime,
    )?;
    heap.array_push(&old_buffer, &[event], &runtime)?;
    call(
        heap,
        module,
        "recordModuleLoadSpan",
        &[
            heap.string("/fixture/b.ts".into())?,
            heap.number(10.0)?,
            heap.number(20.0)?,
            heap.number(8.0)?,
            heap.array(vec![])?,
        ],
        &runtime,
    )?;
    assert_eq!(heap.array_length(&call(heap, module, "openSpanPath", &[], &runtime)?, &runtime)?, 0);
    call(heap, module, "printTimings", &[], &runtime)?;
    let new_buffer = heap.call(&bindings.module_load_buffer, &heap.undefined()?, &[], &runtime)?;
    assert!(!new_buffer.same_identity(&old_buffer));
    assert_eq!(heap.array_length(&new_buffer, &runtime)?, 0);
    assert_eq!(heap.array_length(&old_buffer, &runtime)?, 1);
    let output = probe.lock().map_err(|_| ValueError::PoisonedHeap)?.console[1].clone();
    let output_text = output.to_utf8().map_err(|_| ValueError::InvalidNode)?;
    assert!(output_text.contains("load:a.ts"));
    assert!(output_text.contains("load:b.ts"));
    assert!(output_text.contains("top body/TLA:"));
    assert!(output_text.contains("tree:"));
    assert!(output_text.contains("Total: 95.0ms (since first marker)"));
    call(heap, module, "printTimings", &[], &runtime)?;
    call(heap, module, "endTiming", &[], &runtime)?;
    call(heap, module, "printTimings", &[], &runtime)?;
    assert_eq!(probe.lock().map_err(|_| ValueError::PoisonedHeap)?.clock_calls, 3);
    observe_sink_mutation(heap, module, &runtime)?;
    observe_error_enumeration(heap, globals, &bindings, &runtime)?;
    observe_dynamic_math(heap, globals, &bindings, &runtime)?;
    observe_invalid_directory(heap, globals, &bindings, &runtime)?;
    // Owner expiry is a capability error; preparation is never recreated.
    drop(domain);
    let error = call(heap, module, "startupMarker", &[heap.string("expired".into())?], &runtime).unwrap_err();
    assert!(matches!(error, ValueError::ExpiredCapability(cap) if cap == LOGGER_PORT));
    Ok(LoggerSyncObservation { exports_exercised: 14, console_only_json_calls: 2, timing_output: output })
}

// Extend the existing coarse family with the selected fixed-source correction
// observations. These are controlled graph observations, never a new test family.
static TRACE_RETURN: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Controlled fixed logger correction Get/Call trace",
    name: "traceReturn",
    length: 0,
    form: CompiledFunctionForm::Arrow,
    capture_count: 3,
    body: |heap, captures, _, _, runtime| {
        heap.array_push(&captures[0], &[captures[1].clone()], runtime)?;
        Ok(captures[2].clone())
    },
};
static CONSTANT_RETURN: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Controlled fixed logger Date method",
    name: "datePart",
    length: 0,
    form: CompiledFunctionForm::Arrow,
    capture_count: 1,
    body: |_, captures, _, _, _| Ok(captures[0].clone()),
};
static REPLACE_MATH: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Controlled logger dynamic Math oracle year method",
    name: "getFullYear",
    length: 0,
    form: CompiledFunctionForm::Arrow,
    capture_count: 3,
    body: |heap, captures, _, _, runtime| {
        heap.array_push(&captures[0], &[heap.string("Get:year:replaceMath".into())?], runtime)?;
        heap.assign(&captures[1], "Math".into(), captures[2].clone(), runtime)?;
        heap.number(2026.0)
    },
};
static CALL_ORIGINAL_FLOOR: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Controlled logger dynamic Math oracle original floor",
    name: "floor",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 3,
    body: |heap, captures, _, args, runtime| {
        heap.array_push(&captures[0], &[heap.string("Call:floor".into())?], runtime)?;
        heap.call(&captures[2], &captures[1], args, runtime)
    },
};
static DELETE_ERROR_FIELD: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Controlled logger Error for-in deletion/prototype oracle",
    name: "get a",
    length: 0,
    form: CompiledFunctionForm::Arrow,
    capture_count: 3,
    body: |heap, captures, _, _, runtime| {
        heap.array_push(&captures[2], &[heap.string("Get:a".into())?], runtime)?;
        assert!(heap.delete_own(&captures[0], &"b".into(), runtime)?);
        if !matches!(heap.snapshot(&captures[1])?, ValueSnapshot::Undefined) {
            heap.set_prototype(&captures[0], Some(&captures[1]), runtime)?;
        }
        heap.string("own-a".into())
    },
};
static NAMED_SINK: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Controlled logger Set live iteration label recorder",
    name: "namedSink",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 2,
    body: |heap, captures, _, _, runtime| {
        heap.array_push(&captures[0], &[captures[1].clone()], runtime)?;
        heap.undefined()
    },
};
static MUTATING_SINK: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Controlled logger Set unregister/re-register during iteration",
    name: "mutatingSink",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 4,
    body: |heap, captures, _, _, runtime| {
        heap.array_push(&captures[0], &[heap.string("A".into())?], runtime)?;
        if !heap.truthy(&heap.get(&captures[3], &"changed".into(), runtime)?)? {
            heap.assign(&captures[3], "changed".into(), heap.boolean(true)?, runtime)?;
            let dispose = heap.get(&captures[3], &"disposeB".into(), runtime)?;
            heap.call(&dispose, &heap.undefined()?, &[], runtime)?;
            let renewed = call(heap, &captures[1], "registerLogSink", &[captures[2].clone()], runtime)?;
            heap.assign(&captures[3], "disposeB".into(), renewed, runtime)?;
        }
        heap.undefined()
    },
};
fn getter(
    heap: &Arc<ValueHeap>,
    owner: &ValueHandle,
    name: &str,
    function: ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<()> {
    let ValueSnapshot::Function(target) = heap.snapshot(&function)? else {
        return Err(ValueError::WrongRuntimeReply("Controlled logger getter target"));
    };
    heap.define_own(
        owner,
        name.into(),
        PropertyDescriptor::Accessor { get: Some(target), set: None, enumerable: true, configurable: true },
        runtime,
    )
}
fn traced_return(heap: &Arc<ValueHeap>, trace: &ValueHandle, label: &str, result: ValueHandle) -> Result<ValueHandle> {
    heap.fresh_compiled_function(&TRACE_RETURN, &[trace.clone(), heap.string(label.into())?, result])
}
fn trace_strings(heap: &Arc<ValueHeap>, trace: &ValueHandle, runtime: &dyn ValueRuntime) -> Result<Vec<WireString>> {
    let mut result = Vec::new();
    for index in 0..heap.array_length(trace, runtime)? {
        let ValueSnapshot::String(text) =
            heap.snapshot(&heap.get(trace, &WireString::from(index.to_string()).into(), runtime)?)?
        else {
            return Err(ValueError::WrongRuntimeReply("Controlled logger trace string"));
        };
        result.push(text);
    }
    Ok(result)
}
fn observe_sink_mutation(heap: &Arc<ValueHeap>, module: &ValueHandle, runtime: &dyn ValueRuntime) -> Result<()> {
    let trace = heap.array(vec![])?;
    let state = object(heap, &[("changed", heap.boolean(false)?)], runtime)?;
    let b = heap.fresh_compiled_function(&NAMED_SINK, &[trace.clone(), heap.string("B".into())?])?;
    let c = heap.fresh_compiled_function(&NAMED_SINK, &[trace.clone(), heap.string("C".into())?])?;
    let a = heap.fresh_compiled_function(&MUTATING_SINK, &[trace.clone(), module.clone(), b.clone(), state.clone()])?;
    let dispose_a = call(heap, module, "registerLogSink", &[a], runtime)?;
    let dispose_b = call(heap, module, "registerLogSink", &[b], runtime)?;
    heap.define_own_data(&state, "disposeB".into(), dispose_b, runtime)?;
    let dispose_c = call(heap, module, "registerLogSink", &[c], runtime)?;
    call(heap, module, "info", &[heap.string("Set mutation".into())?], runtime)?;
    assert_eq!(trace_strings(heap, &trace, runtime)?, vec![WireString::from("A"), "C".into(), "B".into()]);
    let renewed_b = heap.get(&state, &"disposeB".into(), runtime)?;
    for disposer in [dispose_a, renewed_b, dispose_c] {
        heap.call(&disposer, &heap.undefined()?, &[], runtime)?;
    }
    call(heap, module, "info", &[heap.string("Set cleared".into())?], runtime)?;
    assert_eq!(heap.array_length(&trace, runtime)?, 3);
    Ok(())
}
fn observe_error_enumeration(
    heap: &Arc<ValueHeap>,
    globals: &NativeGlobals,
    bindings: &NativeLoggerBindings,
    runtime: &dyn ValueRuntime,
) -> Result<()> {
    let error_prototype = heap.get(&globals.errors.error, &"prototype".into(), runtime)?;
    let replacer = heap.get(bindings.module_cells(), &"jsonReplacer".into(), runtime)?;
    for kind in ["hidden", "enumerable", "changedPrototype"] {
        let prototype = heap.object_with_prototype(Some(&error_prototype))?;
        heap.define_own(
            &prototype,
            "b".into(),
            PropertyDescriptor::Data {
                value: heap.string("old-inherited".into())?,
                writable: false,
                enumerable: kind == "enumerable",
                configurable: true,
            },
            runtime,
        )?;
        let value = heap.object_with_prototype(Some(&prototype))?;
        let next = if kind == "changedPrototype" {
            let next = heap.object_with_prototype(Some(&error_prototype))?;
            heap.define_own_data(&next, "c".into(), heap.string("new-inherited".into())?, runtime)?;
            next
        } else {
            heap.undefined()?
        };
        let trace = heap.array(vec![])?;
        getter(
            heap,
            &value,
            "a",
            heap.fresh_compiled_function(&DELETE_ERROR_FIELD, &[value.clone(), next, trace.clone()])?,
            runtime,
        )?;
        heap.define_own_data(&value, "b".into(), heap.string("own-b".into())?, runtime)?;
        let output = heap.call(&replacer, &heap.undefined()?, &[heap.string("".into())?, value], runtime)?;
        assert_eq!(heap.to_string(&heap.get(&output, &"a".into(), runtime)?, runtime)?, WireString::from("own-a"));
        if kind == "enumerable" {
            assert_eq!(
                heap.to_string(&heap.get(&output, &"b".into(), runtime)?, runtime)?,
                WireString::from("old-inherited")
            );
        } else {
            assert!(heap.own_descriptor(&output, &"b".into(), runtime)?.is_none());
        }
        assert!(heap.own_descriptor(&output, &"c".into(), runtime)?.is_none());
        assert_eq!(trace_strings(heap, &trace, runtime)?, vec![WireString::from("Get:a")]);
    }
    Ok(())
}
fn observe_dynamic_math(
    heap: &Arc<ValueHeap>,
    globals: &NativeGlobals,
    bindings: &NativeLoggerBindings,
    runtime: &dyn ValueRuntime,
) -> Result<()> {
    let trace = heap.array(vec![])?;
    let original_math = heap.get(&globals.environment, &"Math".into(), runtime)?;
    let original_floor = heap.get(&original_math, &"floor".into(), runtime)?;
    let absolute = heap.object()?;
    heap.define_own_data(
        &absolute,
        "valueOf".into(),
        traced_return(heap, &trace, "ToNumber:absolute", heap.number(480.0)?)?,
        runtime,
    )?;
    let abs = traced_return(heap, &trace, "Call:abs", absolute)?;
    let first = heap.object()?;
    getter(heap, &first, "abs", traced_return(heap, &trace, "Get:abs", abs)?, runtime)?;
    let floor =
        heap.fresh_compiled_function(&CALL_ORIGINAL_FLOOR, &[trace.clone(), original_math.clone(), original_floor])?;
    let second = heap.object()?;
    getter(heap, &second, "floor", traced_return(heap, &trace, "Get:floor", floor)?, runtime)?;
    let date = heap.object()?;
    heap.define_own_data(
        &date,
        "getTimezoneOffset".into(),
        traced_return(heap, &trace, "Get:offset", heap.number(-480.0)?)?,
        runtime,
    )?;
    heap.define_own_data(
        &date,
        "getFullYear".into(),
        heap.fresh_compiled_function(&REPLACE_MATH, &[trace.clone(), globals.environment.clone(), second])?,
        runtime,
    )?;
    for (name, number) in [
        ("getMonth", 9.0),
        ("getDate", 7.0),
        ("getHours", 4.0),
        ("getMinutes", 0.0),
        ("getSeconds", 0.0),
        ("getMilliseconds", 0.0),
    ] {
        heap.define_own_data(
            &date,
            name.into(),
            heap.fresh_compiled_function(&CONSTANT_RETURN, &[heap.number(number)?])?,
            runtime,
        )?;
    }
    let timestamp = heap.get(bindings.module_cells(), &"formatLocalTimestamp".into(), runtime)?;
    heap.assign(&globals.environment, "Math".into(), first, runtime)?;
    let result = heap.call(&timestamp, &heap.undefined()?, &[date], runtime);
    heap.assign(&globals.environment, "Math".into(), original_math, runtime)?;
    assert_eq!(heap.to_string(&result?, runtime)?, WireString::from("2026-10-07T04:00:00.000+08:00"));
    assert_eq!(
        trace_strings(heap, &trace, runtime)?,
        [
            "Get:offset",
            "Get:abs",
            "Call:abs",
            "Get:year:replaceMath",
            "Get:floor",
            "ToNumber:absolute",
            "Call:floor",
            "ToNumber:absolute"
        ]
        .map(WireString::from)
        .to_vec()
    );
    Ok(())
}
fn observe_invalid_directory(
    heap: &Arc<ValueHeap>,
    globals: &NativeGlobals,
    bindings: &NativeLoggerBindings,
    runtime: &dyn ValueRuntime,
) -> Result<()> {
    let make_file = heap.get(bindings.module_cells(), &"makeFileTransport".into(), runtime)?;
    for directory in [
        heap.number(1.0)?,
        heap.boolean(true)?,
        heap.object()?,
        heap.array(vec![])?,
        heap.symbol(Some("invalid".into()))?,
    ] {
        let error = heap.call(&make_file, &heap.undefined()?, &[directory], runtime).unwrap_err();
        let ValueError::Thrown(reason) = error else {
            panic!("ordinary invalid directory must retain original TypeError carrier");
        };
        assert!(heap.instance_of(&reason, &globals.errors.type_error, runtime)?);
    }
    Ok(())
}
