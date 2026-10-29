//! Selected actual compiled logger.time observations in the existing realm
//! coarse entrance. Clock/stderr faults are explicitly controlled graph imports;
//! this is not real system stderr or full production Logger acceptance.

use ara_cli::{
    native_logger::time::{NativeLoggerTimeImports, NativeLoggerTimeModule},
    native_realm::{NativeRealm, context::NativeScope, promise::NativeDrain},
};
use ara_rpc::value_heap::{
    CompiledFunctionForm, CompiledFunctionSpec, OpaqueKind, PropertyDescriptor, ValueError, ValueHandle, ValueHeap,
    ValueRuntime, ValueSnapshot,
};
use std::sync::Arc;
type Result<T> = std::result::Result<T, ValueError>;

fn get(heap: &Arc<ValueHeap>, value: &ValueHandle, name: &str, runtime: &dyn ValueRuntime) -> Result<ValueHandle> {
    heap.get(value, &name.into(), runtime)
}
fn set(
    heap: &Arc<ValueHeap>,
    value: &ValueHandle,
    name: &str,
    entry: ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<()> {
    heap.define_own_data(value, name.into(), entry, runtime)
}
fn number(heap: &Arc<ValueHeap>, value: &ValueHandle) -> Result<f64> {
    let ValueSnapshot::Number(value) = heap.snapshot(value)? else { panic!("controlled Number expected") };
    Ok(value)
}
fn text(heap: &Arc<ValueHeap>, value: &ValueHandle) -> Result<String> {
    let ValueSnapshot::String(value) = heap.snapshot(value)? else { panic!("controlled String expected") };
    Ok(value.to_utf8().unwrap())
}
fn len(heap: &Arc<ValueHeap>, value: &ValueHandle, runtime: &dyn ValueRuntime) -> Result<usize> {
    Ok(number(heap, &get(heap, value, "length", runtime)?)? as usize)
}
fn is_undefined(heap: &Arc<ValueHeap>, value: &ValueHandle) -> Result<bool> {
    Ok(matches!(heap.snapshot(value)?, ValueSnapshot::Undefined))
}
fn assert_throw(result: Result<ValueHandle>, reason: &ValueHandle) {
    assert!(matches!(result, Err(ValueError::Thrown(value)) if value.same_identity(reason)));
}
fn getter(
    heap: &Arc<ValueHeap>,
    object: &ValueHandle,
    name: &str,
    function: ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<()> {
    let ValueSnapshot::Function(target) = heap.snapshot(&function)? else { panic!("compiled getter expected") };
    heap.define_own(
        object,
        name.into(),
        PropertyDescriptor::Accessor { get: Some(target), set: None, enumerable: true, configurable: true },
        runtime,
    )
}

static RETURN: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "controlled wrapped function original return and arguments",
    name: "wrapped",
    length: 0,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 1,
    body: |heap, captures, receiver, _, _| {
        assert!(is_undefined(heap, receiver)?);
        Ok(captures[0].clone())
    },
};
static THROW: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "controlled wrapped function retains original exception identity",
    name: "wrappedThrow",
    length: 0,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 1,
    body: |_, captures, _, _, _| Err(ValueError::Thrown(captures[0].clone())),
};
static FORWARDED_ARGUMENTS: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "controlled original argument-spread order and identities",
    name: "forwardedArguments",
    length: 2,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 2,
    body: |heap, captures, receiver, args, _| {
        assert!(is_undefined(heap, receiver)?);
        assert_eq!(args.len(), 2);
        assert!(args[0].same_identity(&captures[0]));
        assert!(args[1].same_identity(&captures[1]));
        Ok(args[0].clone())
    },
};
static GET_THROW: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "controlled public then getter count and original thrown reason",
    name: "get then",
    length: 0,
    form: CompiledFunctionForm::Builtin,
    capture_count: 2,
    body: |heap, captures, _, _, runtime| {
        heap.array_push(&captures[0], &[heap.number(1.0)?], runtime)?;
        Err(ValueError::Thrown(captures[1].clone()))
    },
};

static CONTROLLED_NOW: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "controlled real compiled performance.now binding with explicit failure sequence and getStore consumer",
    name: "now",
    length: 0,
    form: CompiledFunctionForm::Builtin,
    capture_count: 2,
    body: |heap, captures, receiver, args, runtime| {
        assert!(receiver.same_identity(&captures[1]));
        assert!(args.is_empty());
        let state = &captures[0];
        let tick = number(heap, &get(heap, state, "ticks", runtime)?)? + 1.0;
        set(heap, state, "ticks", heap.number(tick)?, runtime)?;
        let storage = get(heap, state, "storage", runtime)?;
        let store = if is_undefined(heap, &storage)? {
            heap.undefined()?
        } else {
            let method = get(heap, &storage, "getStore", runtime)?;
            heap.call(&method, &storage, &[], runtime)?
        };
        let scopes = get(heap, state, "scopes", runtime)?;
        heap.array_push(&scopes, &[store], runtime)?;
        let first = number(heap, &get(heap, state, "fail1", runtime)?)?;
        let second = number(heap, &get(heap, state, "fail2", runtime)?)?;
        if tick == first || tick == second {
            if heap.truthy(&get(heap, state, "infra", runtime)?)? {
                return Err(ValueError::UnsupportedOperation("Controlled performance Host failure"));
            }
            return Err(ValueError::Thrown(get(
                heap,
                state,
                if tick == first { "reason1" } else { "reason2" },
                runtime,
            )?));
        }
        heap.number(tick)
    },
};
static CONTROLLED_WRITE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "controlled synchronous fs.writeSync import preserves stderr fd/text and exception versus infrastructure",
    name: "writeSync",
    length: 2,
    form: CompiledFunctionForm::Builtin,
    capture_count: 3,
    body: |heap, captures, receiver, args, runtime| {
        assert!(receiver.same_identity(&captures[2]));
        assert_eq!(args.len(), 2);
        assert_eq!(number(heap, &args[0])?, 2.0);
        heap.array_push(&captures[0], &[args[1].clone()], runtime)?;
        let fault = get(heap, &captures[1], "writeFault", runtime)?;
        match number(heap, &fault)? as u8 {
            1 => Err(ValueError::Thrown(get(heap, &captures[1], "reason1", runtime)?)),
            2 => Err(ValueError::UnsupportedOperation("Controlled stderr Host failure")),
            _ => heap.number(0.0),
        }
    },
};
static CUSTOM_THEN: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "controlled genuine Promise dynamic then retains actual handlers and returns original non-Promise identity",
    name: "then",
    length: 2,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 3,
    body: |heap, captures, receiver, args, runtime| {
        assert!(receiver.same_identity(&captures[0]));
        assert_eq!(args.len(), 2);
        heap.array_push(&captures[1], args, runtime)?;
        Ok(captures[2].clone())
    },
};
static CALL_TIME_CALLBACK: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "controlled argument-order adapter calls actual logger.time with original op and operation-local function",
    name: "invokeTime",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 2,
    body: |heap, captures, _, args, runtime| {
        heap.call(&captures[0], &heap.undefined()?, &[captures[1].clone(), args[0].clone()], runtime)
    },
};
static AFTER_AWAIT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "controlled persistent native-await continuation consumes actual logger.getStore/time then settles execute-result",
    name: "afterAwait",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 8,
    body: |heap, captures, _, args, runtime| {
        assert_eq!(args.len(), 1);
        assert!(args[0].same_identity(&captures[7]));
        let store = heap.call(&captures[2], &captures[1], &[], runtime)?;
        assert!(store.same_identity(&captures[3]));
        let children = get(heap, &store, "children", runtime)?;
        let before = len(heap, &children, runtime)?;
        // Real original time body adds a point to this exact after-await span.
        heap.call(&captures[0], &heap.undefined()?, &[captures[6].clone()], runtime)?;
        assert_eq!(len(heap, &children, runtime)?, before + 1);
        let point = get(heap, &children, &before.to_string(), runtime)?;
        assert!(get(heap, &point, "parent", runtime)?.same_identity(&store));
        heap.array_push(&captures[4], &[captures[6].clone()], runtime)?;
        heap.call(&captures[5], &heap.undefined()?, args, runtime)?;
        heap.undefined()
    },
};

struct ControlledModule {
    module: NativeLoggerTimeModule,
    process: ValueHandle,
    env: ValueHandle,
    state: ValueHandle,
    writes: ValueHandle,
    scopes: ValueHandle,
}
fn controlled(realm: &NativeRealm) -> Result<ControlledModule> {
    let heap = realm.values.heap();
    let runtime = realm.values.runtime();
    let state = heap.object()?;
    let writes = heap.array(Vec::new())?;
    let scopes = heap.array(Vec::new())?;
    for (name, value) in [
        ("ticks", heap.number(0.0)?),
        ("fail1", heap.number(-1.0)?),
        ("fail2", heap.number(-1.0)?),
        ("infra", heap.boolean(false)?),
        ("writeFault", heap.number(0.0)?),
        ("storage", heap.undefined()?),
        ("scopes", scopes.clone()),
        ("reason1", heap.object()?),
        ("reason2", heap.object()?),
    ] {
        set(heap, &state, name, value, runtime)?;
    }
    let performance = heap.object()?;
    set(
        heap,
        &performance,
        "now",
        heap.fresh_compiled_function(&CONTROLLED_NOW, &[state.clone(), performance.clone()])?,
        runtime,
    )?;
    let fs = heap.object()?;
    set(
        heap,
        &fs,
        "writeSync",
        heap.fresh_compiled_function(&CONTROLLED_WRITE, &[writes.clone(), state.clone(), fs.clone()])?,
        runtime,
    )?;
    let process = heap.object()?;
    let env = heap.object()?;
    set(heap, &process, "env", env.clone(), runtime)?;
    let module =
        NativeLoggerTimeModule::prepare(realm, NativeLoggerTimeImports { performance, process: process.clone(), fs })?;
    set(heap, &state, "storage", module.span_storage().clone(), runtime)?;
    Ok(ControlledModule { module, process, env, state, writes, scopes })
}
fn invoke(
    heap: &Arc<ValueHeap>,
    module: &NativeLoggerTimeModule,
    op: &str,
    function: &ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    heap.call(&module.time, &heap.undefined()?, &[heap.string(op.into())?, function.clone()], runtime)
}
fn ticks(heap: &Arc<ValueHeap>, state: &ValueHandle, runtime: &dyn ValueRuntime) -> Result<f64> {
    number(heap, &get(heap, state, "ticks", runtime)?)
}
fn last_child(heap: &Arc<ValueHeap>, root: &ValueHandle, runtime: &dyn ValueRuntime) -> Result<ValueHandle> {
    let children = get(heap, root, "children", runtime)?;
    get(heap, &children, &(len(heap, &children, runtime)? - 1).to_string(), runtime)
}

pub fn observe(realm: &Arc<NativeRealm>) -> Result<Vec<ValueHandle>> {
    let heap = realm.values.heap();
    let base = realm.values.runtime();
    let undefined = heap.undefined()?;
    let fixture = controlled(realm)?;
    let module = &fixture.module;
    let then_reads = heap.array(Vec::new())?;
    let original_throw = heap.object()?;
    let thenable = heap.object()?;
    getter(
        heap,
        &thenable,
        "then",
        heap.fresh_compiled_function(&GET_THROW, &[then_reads.clone(), original_throw.clone()])?,
        base,
    )?;
    let return_thenable = heap.fresh_compiled_function(&RETURN, &[thenable.clone()])?;
    assert!(invoke(heap, module, "fast", &return_thenable, base)?.same_identity(&thenable));
    let first_arg = heap.object()?;
    let second_arg = heap.object()?;
    let forwarded = heap.fresh_compiled_function(&FORWARDED_ARGUMENTS, &[first_arg.clone(), second_arg.clone()])?;
    assert!(
        heap.call(
            &module.time,
            &undefined,
            &[heap.string("fast-arguments".into())?, forwarded.clone(), first_arg.clone(), second_arg.clone(),],
            base
        )?
        .same_identity(&first_arg)
    );
    assert_eq!(ticks(heap, &fixture.state, base)?, 0.0);
    assert_eq!(len(heap, &then_reads, base)?, 0);
    assert!(is_undefined(
        heap,
        &heap.call(&module.time, &undefined, &[heap.string("unrecorded-point".into())?], base)?
    )?);
    assert_eq!(len(heap, &fixture.writes, base)?, 0);
    set(heap, &fixture.env, "PI_DEBUG_STARTUP", heap.string("1".into())?, base)?;
    heap.call(&module.startup_marker, &undefined, &[heap.string("visible".into())?], base)?;
    assert_eq!(text(heap, &get(heap, &fixture.writes, "0", base)?)?, "[startup] visible\n");
    let replaced_env = heap.object()?;
    set(heap, &fixture.process, "env", replaced_env, base)?;
    heap.call(&module.startup_marker, &undefined, &[heap.string("disabled-live-env".into())?], base)?;
    assert_eq!(len(heap, &fixture.writes, base)?, 1);
    set(heap, &fixture.process, "env", fixture.env.clone(), base)?;
    set(heap, &fixture.state, "writeFault", heap.number(1.0)?, base)?;
    assert!(is_undefined(
        heap,
        &heap.call(&module.startup_marker, &undefined, &[heap.string("best-effort".into())?], base)?
    )?);
    set(heap, &fixture.state, "writeFault", heap.number(2.0)?, base)?;
    assert!(matches!(
        heap.call(&module.startup_marker, &undefined, &[heap.string("infra".into())?], base),
        Err(ValueError::UnsupportedOperation(_))
    ));
    set(heap, &fixture.state, "writeFault", heap.number(0.0)?, base)?;
    let debug_writes_before = len(heap, &fixture.writes, base)?;
    assert!(invoke(heap, module, "debug-only", &return_thenable, base)?.same_identity(&thenable));
    assert_eq!(len(heap, &fixture.writes, base)?, debug_writes_before + 2);
    assert_eq!(
        text(heap, &get(heap, &fixture.writes, &debug_writes_before.to_string(), base)?)?,
        "[startup] debug-only:start\n"
    );
    assert_eq!(
        text(heap, &get(heap, &fixture.writes, &(debug_writes_before + 1).to_string(), base)?)?,
        "[startup] debug-only:done\n"
    );
    assert_eq!(ticks(heap, &fixture.state, base)?, 0.0);
    assert_eq!(len(heap, &then_reads, base)?, 0);

    heap.call(&module.start_timing, &undefined, &[], base)?;
    let root = module.root_span(base)?;
    heap.call(&module.start_timing, &undefined, &[], base)?;
    assert!(module.root_span(base)?.same_identity(&root));
    assert_eq!(ticks(heap, &fixture.state, base)?, 1.0);
    assert_eq!(len(heap, &get(heap, &root, "children", base)?, base)?, 0);
    assert!(is_undefined(heap, &get(heap, &root, "parent", base)?)?);
    let original_logger = heap.object()?;
    module.install_into(&original_logger, base)?;
    assert!(get(heap, &original_logger, "time", base)?.same_identity(&module.time));
    assert!(get(heap, &original_logger, "startTiming", base)?.same_identity(&module.start_timing));
    assert!(get(heap, &original_logger, "startupMarker", base)?.same_identity(&module.startup_marker));

    let mut run = realm.begin_promise_run()?;
    let admission = run.admission();
    let root_scope = admission.root_scope();
    let root_parent = admission.root_parent();
    let runtime = admission.scoped(root_scope, root_parent)?;
    assert!(
        heap.call(
            &module.time,
            &undefined,
            &[heap.string("recorded-arguments".into())?, forwarded, first_arg.clone(), second_arg,],
            &runtime
        )?
        .same_identity(&first_arg)
    );
    assert!(invoke(heap, module, "plain-thenable", &return_thenable, &runtime)?.same_identity(&thenable));
    assert_eq!(len(heap, &then_reads, &runtime)?, 0);
    let plain_span = last_child(heap, &root, &runtime)?;
    assert!(get(heap, &plain_span, "parent", &runtime)?.same_identity(&root));
    assert!(!is_undefined(heap, &get(heap, &plain_span, "end", &runtime)?)?);
    let is_promise = realm.promise().prepare_is_promise(heap)?;
    assert!(!heap.truthy(&heap.call(&is_promise, &undefined, &[thenable], &runtime)?)?);
    let external = heap.opaque(OpaqueKind::Object, ara_rpc::value_heap::CapabilityId(9_742_811), 1)?;
    assert!(matches!(
        heap.call(&is_promise, &undefined, &[external], &runtime),
        Err(ValueError::UnsupportedOperation(_))
    ));

    let dynamic = admission.prepare_pending(root_scope, root_parent)?;
    let handlers = heap.array(Vec::new())?;
    let returned = heap.object()?;
    set(
        heap,
        &dynamic.promise,
        "then",
        heap.fresh_compiled_function(&CUSTOM_THEN, &[dynamic.promise.clone(), handlers.clone(), returned.clone()])?,
        &runtime,
    )?;
    let return_dynamic = heap.fresh_compiled_function(&RETURN, &[dynamic.promise.clone()])?;
    assert!(heap.truthy(&heap.call(&is_promise, &undefined, &[dynamic.promise.clone()], &runtime)?)?);
    assert!(invoke(heap, module, "dynamic-then", &return_dynamic, &runtime)?.same_identity(&returned));
    let dynamic_span = last_child(heap, &root, &runtime)?;
    assert!(is_undefined(heap, &get(heap, &dynamic_span, "end", &runtime)?)?);
    assert_eq!(len(heap, &handlers, &runtime)?, 2);
    let fulfilled = get(heap, &handlers, "0", &runtime)?;
    let rejected = get(heap, &handlers, "1", &runtime)?;
    let value = heap.object()?;
    assert!(heap.call(&fulfilled, &undefined, &[value.clone()], &runtime)?.same_identity(&value));
    assert!(!is_undefined(heap, &get(heap, &dynamic_span, "end", &runtime)?)?);
    assert_throw(heap.call(&rejected, &undefined, &[original_throw.clone()], &runtime), &original_throw);
    getter(
        heap,
        &dynamic.promise,
        "then",
        heap.fresh_compiled_function(&GET_THROW, &[then_reads.clone(), original_throw.clone()])?,
        &runtime,
    )?;
    assert_throw(invoke(heap, module, "throwing-then", &return_dynamic, &runtime), &original_throw);
    assert_eq!(len(heap, &then_reads, &runtime)?, 1);
    let throw_fn = heap.fresh_compiled_function(&THROW, &[original_throw.clone()])?;
    assert_throw(invoke(heap, module, "synchronous-throw", &throw_fn, &runtime), &original_throw);
    assert!(!is_undefined(heap, &get(heap, &last_child(heap, &root, &runtime)?, "end", &runtime)?)?);

    let reason1 = get(heap, &fixture.state, "reason1", &runtime)?;
    let reason2 = get(heap, &fixture.state, "reason2", &runtime)?;
    let sync_return = heap.fresh_compiled_function(&RETURN, &[value.clone()])?;
    let before = ticks(heap, &fixture.state, &runtime)?;
    set(heap, &fixture.state, "fail1", heap.number(before + 2.0)?, &runtime)?;
    assert_throw(invoke(heap, module, "finish-done-fails", &sync_return, &runtime), &reason1);
    assert_eq!(ticks(heap, &fixture.state, &runtime)?, before + 3.0);
    let before = ticks(heap, &fixture.state, &runtime)?;
    set(heap, &fixture.state, "fail1", heap.number(before + 2.0)?, &runtime)?;
    set(heap, &fixture.state, "fail2", heap.number(before + 3.0)?, &runtime)?;
    assert_throw(invoke(heap, module, "both-finishes-fail", &sync_return, &runtime), &reason2);
    assert_eq!(ticks(heap, &fixture.state, &runtime)?, before + 3.0);
    let before = ticks(heap, &fixture.state, &runtime)?;
    set(heap, &fixture.state, "fail1", heap.number(before + 2.0)?, &runtime)?;
    set(heap, &fixture.state, "fail2", heap.number(-1.0)?, &runtime)?;
    assert_throw(invoke(heap, module, "fn-error-replaced-by-finish", &throw_fn, &runtime), &reason1);
    let before = ticks(heap, &fixture.state, &runtime)?;
    set(heap, &fixture.state, "fail1", heap.number(before + 1.0)?, &runtime)?;
    assert_throw(heap.call(&rejected, &undefined, &[original_throw.clone()], &runtime), &reason1);
    let before = ticks(heap, &fixture.state, &runtime)?;
    set(heap, &fixture.state, "fail1", heap.number(before + 2.0)?, &runtime)?;
    set(heap, &fixture.state, "infra", heap.boolean(true)?, &runtime)?;
    assert!(matches!(
        invoke(heap, module, "finish-infrastructure", &sync_return, &runtime),
        Err(ValueError::UnsupportedOperation(_))
    ));
    assert_eq!(ticks(heap, &fixture.state, &runtime)?, before + 2.0);
    set(heap, &fixture.state, "fail1", heap.number(-1.0)?, &runtime)?;
    set(heap, &fixture.state, "infra", heap.boolean(false)?, &runtime)?;
    // Execute the original rejection handler through actual public Promise
    // scheduling too. Its finish failure replaces rejection without a second
    // source catch/finish attempt, because the time call returned before this.
    let real_rejected = admission.prepare_pending(root_scope, root_parent)?;
    let return_rejected = heap.fresh_compiled_function(&RETURN, &[real_rejected.promise.clone()])?;
    let rejected_output = invoke(heap, module, "queued-rejection-finish-fails", &return_rejected, &runtime)?;
    let before = ticks(heap, &fixture.state, &runtime)?;
    set(heap, &fixture.state, "fail1", heap.number(before + 1.0)?, &runtime)?;
    heap.call(&real_rejected.reject, &undefined, &[original_throw.clone()], &runtime)?;
    assert!(matches!(run.drain_one()?, NativeDrain::RanJob { .. }));
    assert_eq!(run.drain_one()?, NativeDrain::Idle);
    assert_eq!(ticks(heap, &fixture.state, &runtime)?, before + 1.0);
    let ara_cli::native_realm::promise::NativePromiseState::Rejected(reason) =
        realm.promise().snapshot(heap, &rejected_output)?
    else {
        panic!("actual queued finish failure must reject wrapper Promise")
    };
    assert!(heap.restore_deferred(&reason)?.same_identity(&reason1));
    set(heap, &fixture.state, "fail1", heap.number(-1.0)?, &runtime)?;
    let invalid = invoke(heap, module, "not-callable", &heap.null()?, &runtime).unwrap_err();
    let ValueError::Thrown(native_error) = invalid else { panic!("original realm TypeError required") };
    assert_eq!(text(heap, &get(heap, &native_error, "name", &runtime)?)?, "TypeError");
    assert!(heap.prototype(&native_error, &runtime)?.unwrap().same_identity(&get(
        heap,
        &realm.globals.errors.type_error,
        "prototype",
        &runtime
    )?,));

    // Real Run private await scheduling, with two sibling inner logger spans.
    // The persistent continuation consumes getStore and the original time body,
    // then resolves the authoritative execute-result. No identity stand-in is
    // substituted for either logger.finish or the after-await consumer body.
    let kernel_a = admission.prepare_pending(root_scope, root_parent)?;
    let kernel_b = admission.prepare_pending(root_scope, root_parent)?;
    let execute_a = admission.prepare_pending(root_scope, root_parent)?;
    let execute_b = admission.prepare_pending(root_scope, root_parent)?;
    let get_store = get(heap, module.span_storage(), "getStore", &runtime)?;
    let adapter_a =
        heap.fresh_compiled_function(&CALL_TIME_CALLBACK, &[module.time.clone(), heap.string("sibling-a".into())?])?;
    let adapter_b =
        heap.fresh_compiled_function(&CALL_TIME_CALLBACK, &[module.time.clone(), heap.string("sibling-b".into())?])?;
    let adapter_outer =
        heap.fresh_compiled_function(&CALL_TIME_CALLBACK, &[module.time.clone(), heap.string("outer".into())?])?;
    let mut scope_a: Option<NativeScope> = None;
    let mut scope_b: Option<NativeScope> = None;
    let mut span_a: Option<ValueHandle> = None;
    let mut span_b: Option<ValueHandle> = None;
    let mut output_a: Option<ValueHandle> = None;
    let mut output_b: Option<ValueHandle> = None;
    let mut outer_body = |_: &ValueHandle, _: &[ValueHandle], outer: &dyn ValueRuntime| {
        let outer_span = heap.call(&get_store, module.span_storage(), &[], outer)?;
        assert!(!is_undefined(heap, &outer_span)?);
        let mut start_a = |_: &ValueHandle, _: &[ValueHandle], inner: &dyn ValueRuntime| {
            scope_a = Some(admission.capture_scope(inner)?);
            span_a = Some(heap.call(&get_store, module.span_storage(), &[], inner)?);
            heap.call(&module.time, &undefined, &[heap.string("before-await-a".into())?], inner)?;
            Ok(execute_a.promise.clone())
        };
        output_a = Some(heap.call_with_local_callback(&adapter_a, &undefined, &mut start_a, &[], outer)?);
        let mut start_b = |_: &ValueHandle, _: &[ValueHandle], inner: &dyn ValueRuntime| {
            scope_b = Some(admission.capture_scope(inner)?);
            span_b = Some(heap.call(&get_store, module.span_storage(), &[], inner)?);
            heap.call(&module.time, &undefined, &[heap.string("before-await-b".into())?], inner)?;
            Ok(execute_b.promise.clone())
        };
        output_b = Some(heap.call_with_local_callback(&adapter_b, &undefined, &mut start_b, &[], outer)?);
        assert!(heap.call(&get_store, module.span_storage(), &[], outer)?.same_identity(&outer_span));
        heap.undefined()
    };
    heap.call_with_local_callback(&adapter_outer, &undefined, &mut outer_body, &[], &runtime)?;
    drop(outer_body);
    let outer_span = last_child(heap, &root, &runtime)?;
    let span_a = span_a.unwrap();
    let span_b = span_b.unwrap();
    assert!(!span_a.same_identity(&span_b));
    assert!(get(heap, &span_a, "parent", &runtime)?.same_identity(&outer_span));
    assert!(get(heap, &span_b, "parent", &runtime)?.same_identity(&outer_span));
    let outer_children = get(heap, &outer_span, "children", &runtime)?;
    assert_eq!(len(heap, &outer_children, &runtime)?, 2);
    assert!(get(heap, &outer_children, "0", &runtime)?.same_identity(&span_a));
    assert!(get(heap, &outer_children, "1", &runtime)?.same_identity(&span_b));
    assert!(is_undefined(heap, &get(heap, &span_a, "end", &runtime)?)?);
    assert!(is_undefined(heap, &get(heap, &span_b, "end", &runtime)?)?);
    let continuation_log = heap.array(Vec::new())?;
    let tag_a = heap.object()?;
    let tag_b = heap.object()?;
    let after_a = heap.fresh_compiled_function(
        &AFTER_AWAIT,
        &[
            module.time.clone(),
            module.span_storage().clone(),
            get_store.clone(),
            span_a.clone(),
            continuation_log.clone(),
            execute_a.resolve.clone(),
            heap.string("after-await-a".into())?,
            tag_a.clone(),
        ],
    )?;
    let after_b = heap.fresh_compiled_function(
        &AFTER_AWAIT,
        &[
            module.time.clone(),
            module.span_storage().clone(),
            get_store.clone(),
            span_b.clone(),
            continuation_log.clone(),
            execute_b.resolve.clone(),
            heap.string("after-await-b".into())?,
            tag_b.clone(),
        ],
    )?;
    // Failure continuation is a genuine persistent Throw body, never a string.
    let await_rejected = heap.fresh_compiled_function(&THROW, &[original_throw.clone()])?;
    admission.attach_native_await(&kernel_a.ticket(), &after_a, &await_rejected, scope_a.unwrap(), root_parent)?;
    admission.attach_native_await(&kernel_b.ticket(), &after_b, &await_rejected, scope_b.unwrap(), root_parent)?;
    heap.call(&kernel_a.resolve, &undefined, &[tag_a.clone()], &runtime)?;
    heap.call(&kernel_b.resolve, &undefined, &[tag_b.clone()], &runtime)?;
    let scopes_before = len(heap, &fixture.scopes, &runtime)?;
    let mut jobs = 0;
    while run.drain_one()? != NativeDrain::Idle {
        jobs += 1;
        assert!(jobs < 12, "bounded grouped native logger scheduling");
    }
    assert_eq!(jobs, 4);
    assert_eq!(len(heap, &continuation_log, &runtime)?, 2);
    assert_eq!(text(heap, &get(heap, &continuation_log, "0", &runtime)?)?, "after-await-a");
    assert_eq!(text(heap, &get(heap, &continuation_log, "1", &runtime)?)?, "after-await-b");
    // Each real after-await point sees its own inner scope. finish reactions
    // were registered after storage.run returned, so both see outer scope.
    assert!(get(heap, &fixture.scopes, &scopes_before.to_string(), &runtime)?.same_identity(&span_a));
    assert!(get(heap, &fixture.scopes, &(scopes_before + 1).to_string(), &runtime)?.same_identity(&span_b));
    assert!(get(heap, &fixture.scopes, &(scopes_before + 2).to_string(), &runtime)?.same_identity(&outer_span));
    assert!(get(heap, &fixture.scopes, &(scopes_before + 3).to_string(), &runtime)?.same_identity(&outer_span));
    for (output, tag, span) in [(output_a.unwrap(), tag_a, span_a), (output_b.unwrap(), tag_b, span_b)] {
        let state = realm.promise().snapshot(heap, &output)?;
        let ara_cli::native_realm::promise::NativePromiseState::Fulfilled(value) = state else {
            panic!("finished wrapper Promise expected")
        };
        assert!(heap.restore_deferred(&value)?.same_identity(&tag));
        assert!(!is_undefined(heap, &get(heap, &span, "end", &runtime)?)?);
        assert_eq!(len(heap, &get(heap, &span, "children", &runtime)?, &runtime)?, 2);
    }
    let receipt = heap.object()?;
    set(heap, &receipt, "module", heap.string("actual logger.time selected body".into())?, &runtime)?;
    set(heap, &receipt, "nativeAwaitJobs", heap.number(jobs as f64)?, &runtime)?;
    set(heap, &receipt, "originalTimingWrites", fixture.writes.clone(), &runtime)?;
    set(heap, &receipt, "siblingContinuations", continuation_log, &runtime)?;
    drop(runtime);
    drop(run);
    // Node brand survives revoked Run; it is not a liveness or permission grant.
    assert!(heap.truthy(&heap.call(&is_promise, &undefined, &[dynamic.promise], base)?)?);
    Ok(vec![receipt])
}
