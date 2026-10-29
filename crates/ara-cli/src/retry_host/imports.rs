//! Narrow concrete Native Math/Headers dependencies of fixed OMP retry helpers.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md (MIT).
//! NativeRealm's existing AST Object.entries/Number/Date/Error imports are reused.
use ara_ai::graph_values::GraphValueFactory;
use ara_rpc::value_heap::{
    CompiledFunctionForm, CompiledFunctionSpec, PropertyDescriptor, ValueError, ValueHandle,
    ValueKind, ValueRuntime,
};
use std::sync::Arc;
type Result<T> = std::result::Result<T, ValueError>;

pub(crate) fn install(
    factory: &Arc<GraphValueFactory>,
    globals: &ValueHandle,
    headers: &ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<()> {
    let heap = factory.heap();
    let supplied = heap.get(globals, &"Math".into(), runtime)?;
    let math = if heap.kind(&supplied)? == ValueKind::Undefined {
        let math = heap.object()?;
        define(factory, globals, "Math", math.clone(), runtime)?;
        math
    } else {
        supplied
    };
    for (name, spec) in [("ceil", &CEIL), ("trunc", &TRUNC), ("max", &MAX), ("floor", &FLOOR)] {
        if heap.own_descriptor(&math, &name.into(), runtime)?.is_none() {
            define(
                factory,
                &math,
                name,
                heap.fresh_compiled_function(spec, &[])?,
                runtime,
            )?;
        }
    }
    if heap
        .own_descriptor(globals, &"Headers".into(), runtime)?
        .is_none()
    {
        define(factory, globals, "Headers", headers.clone(), runtime)?;
    }
    // Validate the actual prepared Object dependency, without replacing its
    // identity or patched method. Its compiled implementation already exists.
    let object = heap.get(globals, &"Object".into(), runtime)?;
    let entries = heap.get(&object, &"entries".into(), runtime)?;
    if !matches!(heap.kind(&entries)?, ValueKind::Function) {
        return Err(ValueError::UnsupportedOperation(
            "Original native Object.entries import is unavailable",
        ));
    }
    Ok(())
}
fn define(
    factory: &Arc<GraphValueFactory>,
    target: &ValueHandle,
    name: &str,
    value: ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<()> {
    factory.heap().define_own(
        target,
        name.into(),
        PropertyDescriptor::Data {
            value,
            writable: true,
            enumerable: false,
            configurable: true,
        },
        runtime,
    )
}
static CEIL: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "ECMAScript Math.ceil actual dependency of OMP596f2da retry-after",
    name: "ceil",
    length: 1,
    capture_count: 0,
    form: CompiledFunctionForm::Builtin,
    body: |heap, _, _, args, runtime| {
        heap.number(
            heap.to_number(&args.first().cloned().unwrap_or(heap.undefined()?), runtime)?
                .ceil(),
        )
    },
};
static TRUNC: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "ECMAScript Math.trunc actual dependency of OMP596f2da SDK/idle timeouts",
    name: "trunc",
    length: 1,
    capture_count: 0,
    form: CompiledFunctionForm::Builtin,
    body: |heap, _, _, args, runtime| {
        heap.number(
            heap.to_number(&args.first().cloned().unwrap_or(heap.undefined()?), runtime)?
                .trunc(),
        )
    },
};
static FLOOR: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "ECMAScript Math.floor actual dependency of OMPb91db66e provider in-flight limit",
    name: "floor",
    length: 1,
    capture_count: 0,
    form: CompiledFunctionForm::Builtin,
    body: |heap, _, _, args, runtime| {
        heap.number(
            heap.to_number(&args.first().cloned().unwrap_or(heap.undefined()?), runtime)?
                .floor(),
        )
    },
};
static MAX: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "ECMAScript Math.max actual dependency of OMP596f2da retry-after/idle",
    name: "max",
    length: 2,
    capture_count: 0,
    form: CompiledFunctionForm::Builtin,
    body: |heap, _, _, args, runtime| {
        let mut result = f64::NEG_INFINITY;
        for value in args {
            let number = heap.to_number(value, runtime)?;
            if result.is_nan() || number.is_nan() {
                result = f64::NAN;
            } else if number > result
                || (number == 0.0 && result == 0.0 && !number.is_sign_negative())
            {
                result = number;
            }
        }
        heap.number(result)
    },
};
