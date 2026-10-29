//! Executable Date constructor/internal-slot consumers required by fixed OMP
//! logger.ts and rotating-file.ts. The original global Function is shared by
//! prompt, logger and other realm consumers. Parsing/string call stay explicit.

use super::*;
use crate::native_realm::date::DATE_PORT;

fn port(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    receiver: &ValueHandle,
    operation: &str,
    arguments: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let mut supplied = Vec::with_capacity(arguments.len() + 1);
    supplied.push(heap.string(operation.into())?);
    supplied.extend_from_slice(arguments);
    heap.call(&captures[4], receiver, &supplied, runtime)
}

pub(super) fn prepare(
    heap: &Arc<ValueHeap>,
    prototypes: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let mut captures = prototypes.to_vec();
    captures.push(heap.function(DATE_PORT)?);
    let date = function(heap, &DATE, &captures)?;
    let prototype = heap.get(&date, &"prototype".into(), runtime)?;
    // Same-revision Bun proves Date.prototype is not a Date instance:
    // getTime called on it throws TypeError "Type error". Do not register it.
    for (name, spec) in [
        ("getTime", &GET_TIME),
        ("valueOf", &VALUE_OF),
        ("getFullYear", &GET_FULL_YEAR),
        ("getMonth", &GET_MONTH),
        ("getDate", &GET_DATE),
        ("getHours", &GET_HOURS),
        ("getMinutes", &GET_MINUTES),
        ("getSeconds", &GET_SECONDS),
        ("getMilliseconds", &GET_MILLISECONDS),
        ("getTimezoneOffset", &GET_TIMEZONE_OFFSET),
        ("setDate", &SET_DATE),
        ("toISOString", &TO_ISO_STRING),
        ("toJSON", &TO_JSON),
    ] {
        nonenumerable(heap, &prototype, name, function(heap, spec, &captures)?, runtime)?;
    }
    nonenumerable(heap, &date, "now", function(heap, &NOW, &captures)?, runtime)?;
    Ok(date)
}

static DATE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Native Date original logger no-argument/internal-slot-copy construction",
    name: "Date",
    length: 7,
    form: CompiledFunctionForm::NativeConstructor(construct),
    capture_count: 5,
    body: |_, _, _, _, _| Err(ValueError::UnsupportedOperation("Native Date string-call domain is not installed")),
};
fn construct(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    receiver: &ValueHandle,
    arguments: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    native(heap, &captures[..4], runtime, || {
        let millis = match arguments {
            [] => port(heap, captures, &heap.undefined()?, "now", &[], runtime)?,
            [input] => {
                // Native Date copying reads [[DateValue]] directly; changing
                // valueOf/Symbol.toPrimitive or own properties cannot affect it.
                let copied = port(heap, captures, &heap.undefined()?, "copy", &[input.clone()], runtime)?;
                if heap.kind(&copied)? == ValueKind::Number {
                    copied
                } else {
                    match heap.kind(input)? {
                        ValueKind::Undefined
                        | ValueKind::Null
                        | ValueKind::Boolean
                        | ValueKind::Number
                        | ValueKind::BigInt
                        | ValueKind::Symbol => heap.number(heap.to_number(input, runtime)?)?,
                        _ => {
                            return Err(ValueError::UnsupportedOperation(
                                "Native Date string/default-object/multi-field construction is not installed",
                            ));
                        }
                    }
                }
            }
            _ => return Err(ValueError::UnsupportedOperation("Native Date multi-field construction is not installed")),
        };
        port(heap, captures, receiver, "register", &[millis], runtime)
    })
}

static NOW: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Native Date.now shared Host clock, independent of receiver",
    name: "now",
    length: 0,
    form: CompiledFunctionForm::Builtin,
    capture_count: 5,
    body: |heap, captures, _, _, runtime| {
        native(heap, &captures[..4], runtime, || port(heap, captures, &heap.undefined()?, "now", &[], runtime))
    },
};
macro_rules! getter {
    ($spec:ident, $name:literal) => {
        static $spec: CompiledFunctionSpec = CompiledFunctionSpec {
            source: concat!("Native Date internal-slot ", $name),
            name: $name,
            length: 0,
            form: CompiledFunctionForm::Builtin,
            capture_count: 5,
            body: |heap, captures, receiver, _, runtime| {
                native(heap, &captures[..4], runtime, || port(heap, captures, receiver, $name, &[], runtime))
            },
        };
    };
}
getter!(GET_TIME, "getTime");
getter!(VALUE_OF, "valueOf");
getter!(GET_FULL_YEAR, "getFullYear");
getter!(GET_MONTH, "getMonth");
getter!(GET_DATE, "getDate");
getter!(GET_HOURS, "getHours");
getter!(GET_MINUTES, "getMinutes");
getter!(GET_SECONDS, "getSeconds");
getter!(GET_MILLISECONDS, "getMilliseconds");
getter!(GET_TIMEZONE_OFFSET, "getTimezoneOffset");
getter!(TO_ISO_STRING, "toISOString");
static TO_JSON: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Native Date toJSON object receiver: number-hint ToPrimitive, nonfinite null, dynamic Get/Call toISOString",
    name: "toJSON",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 5,
    body: |heap, captures, receiver, _, runtime| {
        native(heap, &captures[..4], runtime, || {
            // Actual Date/logger receivers are already objects. A general
            // ToObject/boxed primitive domain has not been installed here.
            if !matches!(heap.kind(receiver)?, ValueKind::Object | ValueKind::Array | ValueKind::Function) {
                return Err(ValueError::UnsupportedOperation("Native Date.toJSON primitive boxing is not installed"));
            }
            let primitive = heap.to_primitive(receiver, false, runtime)?;
            if let ValueSnapshot::Number(number) = heap.snapshot(&primitive)? {
                if !number.is_finite() {
                    return heap.null();
                }
            }
            // The original performs this Get after conversion and invokes the
            // current method with the original object and no arguments.
            let method = heap.get(receiver, &"toISOString".into(), runtime)?;
            heap.call(&method, receiver, &[], runtime)
        })
    },
};
static SET_DATE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Native Date setDate: thisTimeValue, ToNumber, local-calendar normalization, same-instance commit",
    name: "setDate",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 5,
    body: |heap, captures, receiver, arguments, runtime| {
        native(heap, &captures[..4], runtime, || {
            // This read must precede even a throwing or reentrant conversion.
            let initial = port(heap, captures, receiver, "getTime", &[], runtime)?;
            let input = argument(heap, arguments, 0)?;
            let day = heap.number(heap.to_number(&input, runtime)?)?;
            port(heap, captures, receiver, "setDate", &[initial, day], runtime)
        })
    },
};
