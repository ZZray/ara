//! Direct Native Object/Set/Symbol and numeric dependencies of original logger.
//! Existing heap entries/Set/Map/descriptor/iterator operations remain unique.
//! Scope: logger-consumed domains, not acceptance of all ECMAScript globals.
use super::*;

fn define(
    heap: &Arc<ValueHeap>,
    target: &ValueHandle,
    key: &str,
    value: ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<()> {
    heap.define_own(
        target,
        key.into(),
        PropertyDescriptor::Data { value, writable: true, enumerable: false, configurable: true },
        runtime,
    )
}
fn native_error(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    runtime: &dyn ValueRuntime,
    error: ValueError,
) -> ValueError {
    let (index, message) = match error {
        ValueError::TypeError(message) => (1, message),
        ValueError::SyntaxError(message) => (2, message),
        ValueError::RangeError(message) => (3, message),
        other => return other,
    };
    let result: Result<ValueHandle> = (|| {
        let value = heap.object_with_prototype(Some(&captures[index]))?;
        define(heap, &value, "message", heap.string(message.into())?, runtime)?;
        Ok(value)
    })();
    match result {
        Ok(value) => ValueError::Thrown(value),
        Err(error) => error,
    }
}
fn argument(heap: &Arc<ValueHeap>, args: &[ValueHandle], i: usize) -> Result<ValueHandle> {
    args.get(i).cloned().map(Ok).unwrap_or_else(|| heap.undefined())
}
fn string(heap: &Arc<ValueHeap>, value: &ValueHandle, runtime: &dyn ValueRuntime) -> Result<WireString> {
    if heap.kind(value)? == ValueKind::Symbol {
        return Err(ValueError::TypeError("Cannot convert a Symbol value to a string".into()));
    }
    heap.to_string(value, runtime)
}
macro_rules! builtin {
    ($spec:ident,$name:literal,$length:expr,$body:ident) => {
        static $spec: CompiledFunctionSpec = CompiledFunctionSpec {
            source: concat!("Native primitive consumed by OMP596f2da logger: ", $name),
            name: $name,
            length: $length,
            form: CompiledFunctionForm::Builtin,
            capture_count: 4,
            body: |heap, captures, receiver, args, runtime| {
                $body(heap, receiver, args, runtime).map_err(|error| native_error(heap, captures, runtime, error))
            },
        };
    };
}
fn define_missing(
    heap: &Arc<ValueHeap>,
    target: &ValueHandle,
    key: &str,
    value: ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<()> {
    if heap.own_descriptor(target, &key.into(), runtime)?.is_none() {
        define(heap, target, key, value, runtime)?;
    }
    Ok(())
}
fn error_prototypes(
    heap: &Arc<ValueHeap>,
    environment: &ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<Vec<ValueHandle>> {
    let mut errors = Vec::new();
    for name in ["Error", "TypeError", "SyntaxError", "RangeError"] {
        errors.push(heap.get(&heap.get(environment, &name.into(), runtime)?, &"prototype".into(), runtime)?);
    }
    Ok(errors)
}
fn install_object(
    heap: &Arc<ValueHeap>,
    environment: &ValueHandle,
    errors: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let object = if heap.own_descriptor(environment, &"Object".into(), runtime)?.is_some() {
        heap.get(environment, &"Object".into(), runtime)?
    } else {
        let object = heap.fresh_compiled_function(&OBJECT, errors)?;
        define(heap, &object, "prototype", heap.object_prototype(), runtime)?;
        define(heap, environment, "Object", object.clone(), runtime)?;
        object
    };
    define_missing(heap, &object, "entries", heap.fresh_compiled_function(&ENTRIES, errors)?, runtime)?;
    Ok(object)
}
fn install_collection(
    heap: &Arc<ValueHeap>,
    environment: &ValueHandle,
    errors: &[ValueHandle],
    name: &str,
    spec: &'static CompiledFunctionSpec,
    instance: ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<()> {
    if heap.own_descriptor(environment, &name.into(), runtime)?.is_some() {
        return Ok(());
    }
    let prototype = heap.prototype(&instance, runtime)?.ok_or(ValueError::InvalidNode)?;
    let constructor = heap.fresh_native_constructor_with_prototype(spec, errors, &prototype)?;
    define(heap, &prototype, "constructor", constructor.clone(), runtime)?;
    define(heap, environment, name, constructor, runtime)
}
/// AST imports only its actual Object/fromEntries/Set dependencies. Preparation
/// preserves already supplied globals and methods, including original patches.
pub(crate) fn install_ast_collections(
    heap: &Arc<ValueHeap>,
    environment: &ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<()> {
    let errors = error_prototypes(heap, environment, runtime)?;
    let object = install_object(heap, environment, &errors, runtime)?;
    define_missing(heap, &object, "fromEntries", heap.fresh_compiled_function(&FROM_ENTRIES, &errors)?, runtime)?;
    install_collection(heap, environment, &errors, "Set", &SET, heap.new_set(&heap.undefined()?, runtime)?, runtime)
}
pub(super) fn install(heap: &Arc<ValueHeap>, environment: &ValueHandle, runtime: &dyn ValueRuntime) -> Result<()> {
    let errors = error_prototypes(heap, environment, runtime)?;
    let object = install_object(heap, environment, &errors, runtime)?;
    define_missing(heap, &object, "assign", heap.fresh_compiled_function(&ASSIGN, &errors)?, runtime)?;
    let symbol = if heap.own_descriptor(environment, &"Symbol".into(), runtime)?.is_some() {
        heap.get(environment, &"Symbol".into(), runtime)?
    } else {
        heap.fresh_compiled_function(&SYMBOL, &errors)?
    };
    define_missing(heap, &symbol, "for", heap.fresh_compiled_function(&SYMBOL_FOR, &errors)?, runtime)?;
    let math = if heap.own_descriptor(environment, &"Math".into(), runtime)?.is_some() {
        heap.get(environment, &"Math".into(), runtime)?
    } else {
        heap.object()?
    };
    for (name, spec) in [("abs", &ABS), ("floor", &FLOOR), ("max", &MAX)] {
        define_missing(heap, &math, name, heap.fresh_compiled_function(spec, &errors)?, runtime)?;
    }
    for (name, value) in [("Symbol", symbol), ("Math", math), ("globalThis", environment.clone())] {
        define_missing(heap, environment, name, value, runtime)?;
    }
    for (name, spec, instance) in [
        ("Set", &SET, heap.new_set(&heap.undefined()?, runtime)?),
        ("Map", &MAP, heap.new_map(&heap.undefined()?, runtime)?),
    ] {
        install_collection(heap, environment, &errors, name, spec, instance, runtime)?;
    }
    let number_prototype = heap.prototype(&heap.number(0.0)?, runtime)?.ok_or(ValueError::InvalidNode)?;
    define_missing(heap, &number_prototype, "toFixed", heap.fresh_compiled_function(&TO_FIXED, &errors)?, runtime)?;
    let string_prototype = heap.prototype(&heap.string("".into())?, runtime)?.ok_or(ValueError::InvalidNode)?;
    define_missing(heap, &string_prototype, "repeat", heap.fresh_compiled_function(&REPEAT, &errors)?, runtime)?;
    Ok(())
}
macro_rules! collection_constructor {
    ($spec:ident,$name:literal,$body:ident,$operation:ident) => {
        fn $body(
            heap: &Arc<ValueHeap>,
            captures: &[ValueHandle],
            receiver: &ValueHandle,
            args: &[ValueHandle],
            runtime: &dyn ValueRuntime,
        ) -> Result<ValueHandle> {
            heap.$operation(receiver, &argument(heap, args, 0)?, runtime)
                .map_err(|error| native_error(heap, captures, runtime, error))
        }
        static $spec: CompiledFunctionSpec = CompiledFunctionSpec {
            source: concat!("Native ", $name, " constructor consumed by fixed logger"),
            name: $name,
            length: 0,
            capture_count: 4,
            form: CompiledFunctionForm::NativeConstructor($body),
            body: |heap, captures, _, _, runtime| {
                Err(native_error(
                    heap,
                    captures,
                    runtime,
                    ValueError::TypeError(concat!("Constructor ", $name, " requires 'new'").into()),
                ))
            },
        };
    };
}
collection_constructor!(SET, "Set", set_construct, initialize_set_receiver);
collection_constructor!(MAP, "Map", map_construct, initialize_map_receiver);
builtin!(OBJECT, "Object", 1, object);
fn object(heap: &Arc<ValueHeap>, _: &ValueHandle, args: &[ValueHandle], _: &dyn ValueRuntime) -> Result<ValueHandle> {
    let value = argument(heap, args, 0)?;
    match heap.kind(&value)? {
        ValueKind::Undefined | ValueKind::Null => heap.object(),
        ValueKind::Object | ValueKind::Array | ValueKind::Function => Ok(value),
        _ => Err(ValueError::UnsupportedOperation(
            "Object primitive boxing is outside the selected logger consumer domain",
        )),
    }
}
builtin!(ASSIGN, "assign", 2, assign);
fn assign(
    heap: &Arc<ValueHeap>,
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let target = argument(heap, args, 0)?;
    if matches!(heap.kind(&target)?, ValueKind::Null | ValueKind::Undefined) {
        return Err(ValueError::TypeError("Cannot convert undefined or null to object".into()));
    }
    if !matches!(heap.kind(&target)?, ValueKind::Object | ValueKind::Array | ValueKind::Function) {
        return Err(ValueError::UnsupportedOperation("Object.assign primitive target boxing is not installed"));
    }
    for source in args.iter().skip(1) {
        if matches!(heap.kind(source)?, ValueKind::Null | ValueKind::Undefined) {
            continue;
        }
        for key in heap.own_keys(source, runtime)? {
            if !heap.own_descriptor(source, &key, runtime)?.is_some_and(|d| d.enumerable()) {
                continue;
            }
            let value = heap.get(source, &key, runtime)?;
            heap.assign(&target, key, value, runtime)?;
        }
    }
    Ok(target)
}
builtin!(ENTRIES, "entries", 1, entries);
fn entries(
    heap: &Arc<ValueHeap>,
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    heap.object_entries(&argument(heap, args, 0)?, runtime)
}
builtin!(FROM_ENTRIES, "fromEntries", 1, from_entries);
fn from_entries(
    heap: &Arc<ValueHeap>,
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let result = heap.object()?;
    let mut iterator = heap.get_iterator(&argument(heap, args, 0)?, runtime)?;
    // An abrupt next propagates directly. Abrupt body completion closes the
    // iterator, preserving the original throw over any return-method throw.
    while let Some(entry) = heap.iterator_next(&mut iterator, runtime)? {
        let body = (|| {
            if !matches!(heap.kind(&entry)?, ValueKind::Object | ValueKind::Array | ValueKind::Function) {
                return Err(ValueError::TypeError("Iterator value is not an entry object".into()));
            }
            let key = heap.get(&entry, &"0".into(), runtime)?;
            let value = heap.get(&entry, &"1".into(), runtime)?;
            let key = heap.to_primitive(&key, true, runtime)?;
            let key = if heap.kind(&key)? == ValueKind::Symbol {
                PropertyKey::Symbol(heap.symbol_key(&key)?)
            } else {
                PropertyKey::String(heap.to_string(&key, runtime)?)
            };
            heap.define_own_data(&result, key, value, runtime)
        })();
        if let Err(error) = body {
            let _ = heap.iterator_close(&mut iterator, runtime);
            return Err(error);
        }
    }
    Ok(result)
}
builtin!(SYMBOL, "Symbol", 0, symbol);
fn symbol(
    heap: &Arc<ValueHeap>,
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let value = argument(heap, args, 0)?;
    let description =
        if heap.kind(&value)? == ValueKind::Undefined { None } else { Some(string(heap, &value, runtime)?) };
    heap.symbol(description)
}
builtin!(SYMBOL_FOR, "for", 1, symbol_for);
fn symbol_for(
    heap: &Arc<ValueHeap>,
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    heap.symbol_for(string(heap, &argument(heap, args, 0)?, runtime)?)
}
builtin!(ABS, "abs", 1, abs);
fn abs(
    heap: &Arc<ValueHeap>,
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    heap.number(heap.to_number(&argument(heap, args, 0)?, runtime)?.abs())
}
builtin!(FLOOR, "floor", 1, floor);
fn floor(
    heap: &Arc<ValueHeap>,
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    heap.number(heap.to_number(&argument(heap, args, 0)?, runtime)?.floor())
}
builtin!(MAX, "max", 2, max);
fn max(
    heap: &Arc<ValueHeap>,
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let mut maximum = f64::NEG_INFINITY;
    for value in args {
        let n = heap.to_number(value, runtime)?;
        if maximum.is_nan() || n.is_nan() {
            maximum = f64::NAN;
        } else if n > maximum || (n == 0.0 && maximum == 0.0 && !n.is_sign_negative()) {
            maximum = n;
        }
    }
    heap.number(maximum)
}
builtin!(TO_FIXED, "toFixed", 1, to_fixed);
fn to_fixed(
    heap: &Arc<ValueHeap>,
    receiver: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ValueSnapshot::Number(n) = heap.snapshot(receiver)? else {
        return Err(ValueError::UnsupportedOperation("Number.toFixed boxed-number domain is not installed"));
    };
    let fraction = argument(heap, args, 0)?;
    let fraction = heap.to_number(&fraction, runtime)?;
    let fraction = if fraction.is_nan() { 0.0 } else { fraction.trunc() };
    if !(0.0..=100.0).contains(&fraction) {
        return Err(ValueError::RangeError("toFixed() digits argument must be between 0 and 100".into()));
    }
    // Fixed original logger invokes only 0/1/2. Wider digits are an explicit
    // dependency gap, never a fabricated rounded output or module completion.
    if fraction > 2.0 {
        return Err(ValueError::UnsupportedOperation(
            "Number.toFixed digits above the logger's 0/1/2 domain are not installed",
        ));
    }
    if !n.is_finite() || n.abs() >= 1e21 {
        return heap.string(heap.to_string(receiver, runtime)?);
    }
    let digits = fraction as u32;
    let text = fixed(n, digits)?;
    heap.string(text.into())
}
builtin!(REPEAT, "repeat", 1, repeat);
fn repeat(
    heap: &Arc<ValueHeap>,
    receiver: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    if matches!(heap.kind(receiver)?, ValueKind::Null | ValueKind::Undefined) {
        return Err(ValueError::TypeError("String.prototype.repeat called on null or undefined".into()));
    }
    let text = string(heap, receiver, runtime)?;
    let n = heap.to_number(&argument(heap, args, 0)?, runtime)?;
    let count = if n.is_nan() { 0.0 } else { n.trunc() };
    if count < 0.0 || !count.is_finite() {
        return Err(ValueError::RangeError("Invalid count value".into()));
    }
    if text.units().is_empty() || count == 0.0 {
        return heap.string("".into());
    }
    if count > usize::MAX as f64 {
        return Err(ValueError::RangeError("Invalid string length".into()));
    }
    let count = count as usize;
    let length = text.units().len().checked_mul(count).ok_or(ValueError::Allocation)?;
    let mut output = Vec::new();
    output.try_reserve(length).map_err(|_| ValueError::Allocation)?;
    for _ in 0..count {
        output.extend_from_slice(text.units());
    }
    heap.string(WireString::from_units(output))
}
/// Exact binary64 x 10^f nearest-integer, ties to larger integer, for f<=2
/// and |x|<1e21. Rust float formatting's ties-to-even is not substituted.
fn fixed(n: f64, digits: u32) -> Result<String> {
    let bits = n.abs().to_bits();
    let field = ((bits >> 52) & 0x7ff) as i32;
    let mantissa = (bits & ((1u64 << 52) - 1)) | if field == 0 { 0 } else { 1u64 << 52 };
    let exponent = if field == 0 { -1074 } else { field - 1023 - 52 } + digits as i32;
    let scaled = u128::from(mantissa) * 5u128.pow(digits);
    let rounded = if exponent >= 0 {
        scaled.checked_shl(exponent as u32).ok_or(ValueError::UnsupportedOperation("Logger toFixed binary exponent"))?
    } else {
        let shift = (-exponent) as u32;
        if shift >= 128 {
            0
        } else {
            let integer = scaled >> shift;
            let remainder = scaled & ((1u128 << shift) - 1);
            integer + u128::from(shift > 0 && remainder >= (1u128 << (shift - 1)))
        }
    };
    let mut text = rounded.to_string();
    if digits > 0 {
        let width = digits as usize + 1;
        if text.len() < width {
            text = format!("{}{}", "0".repeat(width - text.len()), text);
        }
        text.insert(text.len() - digits as usize, '.');
    }
    if n < 0.0 {
        text.insert(0, '-');
    }
    Ok(text)
}
