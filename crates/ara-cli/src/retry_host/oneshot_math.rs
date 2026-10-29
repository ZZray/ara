//! Only missing concrete Math imports selected by fixed oneshot-retry.ts.
//! Preserve supplied original objects/methods and dynamic native conversion.
use crate::native_realm::NativeErrorClasses;
use ara_ai::graph_values::GraphValueFactory;
use ara_rpc::value_heap::{
    CompiledFunctionForm, CompiledFunctionSpec, PropertyDescriptor, ValueError, ValueHandle,
    ValueRuntime,
};
use std::sync::Arc;
type Result<T> = std::result::Result<T, ValueError>;
pub(crate) fn install(
    factory: &Arc<GraphValueFactory>,
    globals: &ValueHandle,
    errors: &NativeErrorClasses,
    runtime: &dyn ValueRuntime,
) -> Result<()> {
    let heap = factory.heap();
    let math = heap.get(globals, &"Math".into(), runtime)?;
    let captures = [
        errors.type_error.clone(),
        errors.syntax_error.clone(),
        errors.range_error.clone(),
    ];
    for (name, spec) in [("min", &MIN), ("round", &ROUND), ("random", &RANDOM)] {
        if heap.own_descriptor(&math, &name.into(), runtime)?.is_none() {
            heap.define_own(
                &math,
                name.into(),
                PropertyDescriptor::Data {
                    value: heap.fresh_compiled_function(spec, &captures)?,
                    writable: true,
                    enumerable: false,
                    configurable: true,
                },
                runtime,
            )?;
        }
    }
    Ok(())
}
fn native_error(
    heap: &Arc<ara_rpc::value_heap::ValueHeap>,
    captures: &[ValueHandle],
    runtime: &dyn ValueRuntime,
    error: ValueError,
) -> ValueError {
    let (index, message) = match error {
        ValueError::TypeError(message) => (0, message),
        ValueError::SyntaxError(message) => (1, message),
        ValueError::RangeError(message) => (2, message),
        other => return other,
    };
    let constructed = heap
        .string(message.into())
        .and_then(|message| heap.construct(&captures[index], &[message], runtime));
    match constructed {
        Ok(value) => ValueError::Thrown(value),
        Err(error) => error,
    }
}
macro_rules! builtin {
    ($name:ident,$text:literal,$length:literal,$body:expr) => {
        static $name: CompiledFunctionSpec = CompiledFunctionSpec {
            source: concat!(
                "Native ECMAScript Math.",
                $text,
                " import for fixed OMP596f2da oneshot-retry.ts"
            ),
            name: $text,
            length: $length,
            capture_count: 3,
            form: CompiledFunctionForm::Builtin,
            body: |heap, captures, _, args, runtime| {
                ($body)(heap, args, runtime)
                    .map_err(|error| native_error(heap, captures, runtime, error))
            },
        };
    };
}
builtin!(MIN, "min", 2, |heap: &Arc<
    ara_rpc::value_heap::ValueHeap,
>,
                         args: &[ValueHandle],
                         runtime: &dyn ValueRuntime|
 -> Result<ValueHandle> {
    let mut minimum = f64::INFINITY;
    // All conversions occur even after a NaN is found, in authored argument order.
    for value in args {
        let number = heap.to_number(value, runtime)?;
        if number.is_nan() || minimum.is_nan() {
            minimum = f64::NAN;
        } else if number < minimum || (number == 0.0 && minimum == 0.0 && number.is_sign_negative())
        {
            minimum = number
        }
    }
    heap.number(minimum)
});
builtin!(ROUND, "round", 1, |heap: &Arc<
    ara_rpc::value_heap::ValueHeap,
>,
                             args: &[ValueHandle],
                             runtime: &dyn ValueRuntime|
 -> Result<ValueHandle> {
    let value = args.first().cloned().unwrap_or(heap.undefined()?);
    let number = heap.to_number(&value, runtime)?;
    let rounded = if !number.is_finite() || number == 0.0 || number.abs() >= 4_503_599_627_370_496.0
    {
        number
    } else if (-0.5..0.0).contains(&number) {
        -0.0
    } else {
        let floor = number.floor();
        if number - floor < 0.5 {
            floor
        } else {
            floor + 1.0
        }
    };
    heap.number(rounded)
});
builtin!(RANDOM, "random", 0, |heap: &Arc<
    ara_rpc::value_heap::ValueHeap,
>,
                               _: &[ValueHandle],
                               _: &dyn ValueRuntime|
 -> Result<ValueHandle> {
    let random = uuid::Uuid::new_v4();
    let mut bytes = [0; 8];
    bytes.copy_from_slice(&random.as_bytes()[8..]);
    heap.number(((u64::from_be_bytes(bytes) & ((1_u64 << 53) - 1)) as f64) / (1_u64 << 53) as f64)
});
