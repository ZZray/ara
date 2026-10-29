//! Draft helper for an existing coarse entrance. Zero new test attributes.
//! Executes actual NativeRealm bodies/source; does not stand for production
//! factory publication, initial/reopen/reset or full OMP realm acceptance.

#[path = "async_context.rs"]
mod async_context;
#[path = "async_promise.rs"]
mod async_promise;
#[path = "json.rs"]
mod json;
#[path = "logger_time.rs"]
mod logger_time;
#[path = "read_continuation.rs"]
mod read_continuation;

use ara_agent::ActualToolExecutionContext;
use ara_cli::{
    actual_tools::{BuiltinToolSource, NativeBuiltinToolHooks},
    native_realm::NativeRealm,
};
use ara_rpc::{
    WireString,
    value_heap::{
        CapabilityId, CompiledFunctionForm, CompiledFunctionSpec, OpaqueKind, OpaqueOperation, PropertyDescriptor,
        RuntimeReply, RuntimeRequest, ValueError, ValueHandle, ValueRuntime, ValueSnapshot,
    },
};
use ara_tools::ToolContext;
use std::{
    path::PathBuf,
    sync::{
        Arc, Barrier,
        atomic::{AtomicUsize, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tokio_util::sync::CancellationToken;

#[path = "date.rs"]
mod date;

type Result<T> = std::result::Result<T, ValueError>;
fn number(realm: &NativeRealm, value: &ValueHandle) -> Result<f64> {
    match realm.values.heap().snapshot(value)? {
        ValueSnapshot::Number(n) => Ok(n),
        _ => Err(ValueError::InvalidNode),
    }
}
fn global(realm: &NativeRealm, name: &str) -> Result<ValueHandle> {
    realm.values.heap().get(&realm.globals.environment, &WireString::from(name).into(), realm.values.runtime())
}
fn method(realm: &NativeRealm, object: &ValueHandle, name: &str) -> Result<ValueHandle> {
    realm.values.heap().get(object, &WireString::from(name).into(), realm.values.runtime())
}
fn thrown(result: Result<ValueHandle>) -> Result<ValueHandle> {
    match result {
        Err(ValueError::Thrown(original)) => Ok(original),
        other => panic!("Expected original native exception, got {other:?}"),
    }
}
fn milliseconds() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as f64
}

/// Use on Root's final prepared production bundle as well as an isolated
/// candidate realm. Callables and exceptions must be the installed originals.
pub fn observe_native_realm_globals(realm: &Arc<NativeRealm>) -> Result<Vec<ValueHandle>> {
    let heap = realm.values.heap();
    let runtime = realm.values.runtime();
    let undefined = heap.undefined()?;
    let mut receipts = Vec::new();
    receipts.extend(json::observe(realm)?);
    let string = global(realm, "String")?;
    assert!(string.same_identity(&realm.globals.string));
    let string_prototype = heap.prototype(&heap.string("".into())?, runtime)?.unwrap();
    assert!(method(realm, &string, "prototype")?.same_identity(&string_prototype));
    assert!(method(realm, &string_prototype, "constructor")?.same_identity(&string));
    assert!(matches!(heap.construct_compiled_function(&string, &[], runtime), Err(ValueError::TypeError(_))));
    for (argument, expected) in [
        (None, WireString::from("")),
        (Some(undefined.clone()), WireString::from("undefined")),
        (Some(heap.number(-0.0)?), WireString::from("0")),
        (Some(heap.string(WireString::from_units(vec![0xd800]))?), WireString::from_units(vec![0xd800])),
        (
            Some(heap.symbol(Some(WireString::from_units(vec![0xd800])))?),
            WireString::from_units(vec![83, 121, 109, 98, 111, 108, 40, 0xd800, 41]),
        ),
    ] {
        let arguments = argument.into_iter().collect::<Vec<_>>();
        let output = heap.call(&string, &undefined, &arguments, runtime)?;
        assert!(matches!(heap.snapshot(&output)?, ValueSnapshot::String(text) if text == expected));
        receipts.push(output);
    }
    static PRIMITIVE: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "controlled String coercion receipt",
        name: "toPrimitive",
        length: 1,
        form: CompiledFunctionForm::Builtin,
        capture_count: 1,
        body: |heap, captures, _, arguments, _| {
            assert!(
                matches!(heap.snapshot(&arguments[0])?, ValueSnapshot::String(text) if text.equals_ascii("string"))
            );
            Ok(captures[0].clone())
        },
    };
    static THROW: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "controlled original String coercion throw receipt",
        name: "toPrimitive",
        length: 1,
        form: CompiledFunctionForm::Builtin,
        capture_count: 1,
        body: |_, captures, _, _, _| Err(ValueError::Thrown(captures[0].clone())),
    };
    let object = heap.object()?;
    heap.define_own_data(
        &object,
        heap.to_primitive_key(),
        heap.fresh_compiled_function(&PRIMITIVE, &[heap.symbol(None)?])?,
        runtime,
    )?;
    let symbol_coercion = thrown(heap.call(&string, &undefined, &[object.clone()], runtime))?;
    assert!(heap.instance_of(&symbol_coercion, &realm.globals.errors.type_error, runtime)?);
    let original_throw = heap.object()?;
    heap.define_own_data(
        &object,
        heap.to_primitive_key(),
        heap.fresh_compiled_function(&THROW, &[original_throw.clone()])?,
        runtime,
    )?;
    assert!(thrown(heap.call(&string, &undefined, &[object], runtime))?.same_identity(&original_throw));
    receipts.push(symbol_coercion);

    let prepared = ara_cli::native_factory::PreparedNativeSession::from_realm(realm.clone(), None)?;
    assert!(prepared.prompt().date().same_identity(&realm.globals.date));
    assert!(Arc::ptr_eq(prepared.values(), &realm.values));
    assert!(prepared.require_tool_hooks().is_err(), "Foundation is not formal factory activation");
    assert!(prepared.require_logger().is_err(), "An absent original logger remains explicit");
    let foreign = NativeRealm::prepare()?;
    assert!(
        ara_cli::native_factory::PreparedNativeSession::from_realm(
            realm.clone(),
            Some(foreign.values.heap().object()?)
        )
        .is_err()
    );
    assert!(prepared.check_values(&foreign.values).is_err());
    let cause = heap.object()?;
    let options = heap.object()?;
    heap.define_own_data(&options, "cause".into(), cause.clone(), runtime)?;
    let original = heap.construct_compiled_function(
        &realm.globals.errors.type_error,
        &[heap.string("native".into())?, options],
        runtime,
    )?;
    assert!(heap.instance_of(&original, &realm.globals.errors.type_error, runtime)?);
    assert!(heap.instance_of(&original, &realm.globals.errors.error, runtime)?);
    assert!(!heap.instance_of(&original, &realm.globals.errors.syntax_error, runtime)?);
    assert!(heap.get(&original, &"cause".into(), runtime)?.same_identity(&cause));
    assert!(matches!(
        heap.own_descriptor(&original, &"message".into(), runtime)?,
        Some(PropertyDescriptor::Data { writable: true, enumerable: false, configurable: true, .. })
    ));
    let to_string = method(realm, &original, "toString")?;
    assert!(
        matches!(heap.snapshot(&heap.call(&to_string,&original,&[],runtime)?)?,ValueSnapshot::String(s) if s.equals_ascii("TypeError: native"))
    );
    assert!(heap.own_descriptor(&to_string, &"prototype".into(), runtime)?.is_none());
    assert!(matches!(heap.construct_compiled_function(&to_string, &[], runtime), Err(ValueError::TypeError(_))));
    let invalid_receiver = thrown(heap.call(&to_string, &undefined, &[], runtime))?;
    assert!(heap.instance_of(&invalid_receiver, &realm.globals.errors.type_error, runtime)?);
    let symbol = heap.symbol(Some("s".into()))?;
    let symbol_error = thrown(heap.call(&realm.globals.errors.error, &undefined, &[symbol], runtime))?;
    assert!(heap.instance_of(&symbol_error, &realm.globals.errors.type_error, runtime)?);
    receipts.extend([original, invalid_receiver, symbol_error]);

    let parse = method(realm, &realm.globals.json, "parse")?;
    let malformed = thrown(heap.call(&parse, &realm.globals.json, &[heap.string("{".into())?], runtime))?;
    assert!(heap.instance_of(&malformed, &realm.globals.errors.syntax_error, runtime)?);
    assert!(heap.instance_of(&malformed, &realm.globals.errors.error, runtime)?);
    let surrogate = heap.call(&parse, &realm.globals.json, &[heap.string(r#""\ud800""#.into())?], runtime)?;
    assert!(matches!(heap.snapshot(&surrogate)?,ValueSnapshot::String(s) if s.units()==[0xd800]));
    receipts.extend([malformed, surrogate]);
    let is_array = method(realm, &realm.globals.array, "isArray")?;
    for (value, expected) in [(heap.array(vec![])?, true), (heap.object()?, false), (undefined.clone(), false)] {
        assert!(
            matches!(heap.snapshot(&heap.call(&is_array,&realm.globals.array,&[value],runtime)?)?,ValueSnapshot::Boolean(b) if b==expected)
        );
    }

    let parse_int = global(realm, "parseInt")?;
    for (text, radix, expected) in [
        ("\u{feff}-0x10junk", 0.0, -16.0),
        ("11", 4_294_967_298.0, 3.0),
        ("9007199254740993suffix", 10.0, 9_007_199_254_740_992.0),
        ("0x10", 10.0, 0.0),
    ] {
        let output = heap.call(&parse_int, &undefined, &[heap.string(text.into())?, heap.number(radix)?], runtime)?;
        assert_eq!(number(realm, &output)?, expected);
        receipts.push(output);
    }
    let negative_zero = heap.call(&parse_int, &undefined, &[heap.string("-000".into())?], runtime)?;
    assert!(heap.object_is(&negative_zero, &heap.number(-0.0)?)?);
    let bad_radix =
        thrown(heap.call(&parse_int, &undefined, &[heap.string("1".into())?, heap.bigint_decimal("10")?], runtime))?;
    assert!(heap.instance_of(&bad_radix, &realm.globals.errors.type_error, runtime)?);
    let parse_float = global(realm, "parseFloat")?;
    for (text, expected) in [("1e+tail", 1.0), (".5suffix", 0.5), ("-Infinityx", f64::NEG_INFINITY)] {
        let output = heap.call(&parse_float, &undefined, &[heap.string(text.into())?], runtime)?;
        assert_eq!(number(realm, &output)?, expected);
        receipts.push(output);
    }
    let mut units = "12.5".encode_utf16().collect::<Vec<_>>();
    units.push(0xd800);
    assert_eq!(
        number(realm, &heap.call(&parse_float, &undefined, &[heap.string(WireString::from_units(units))?], runtime)?)?,
        12.5
    );
    let number_function = global(realm, "Number")?;
    assert_eq!(number(realm, &heap.call(&number_function, &undefined, &[], runtime)?)?, 0.0);
    assert_eq!(
        number(realm, &heap.call(&number_function, &undefined, &[heap.bigint_decimal("9007199254740993")?], runtime)?)?,
        9_007_199_254_740_992.0
    );
    for (text, expected) in [
        ("0x200000000000011", 144_115_188_075_855_904.0),
        ("\u{feff}0B11\u{2029}", 3.0),
        ("0o17", 15.0),
        ("+12.5e1", 125.0),
        (" ", 0.0),
    ] {
        let output = heap.call(&number_function, &undefined, &[heap.string(text.into())?], runtime)?;
        assert_eq!(number(realm, &output)?, expected);
        receipts.push(output);
    }
    for text in ["+0x11", "-0b1", "0x", "0b102", "0o7suffix", "0x1 1", "1e+"] {
        assert!(
            number(realm, &heap.call(&number_function, &undefined, &[heap.string(text.into())?], runtime)?)?.is_nan()
        );
    }
    let is_finite = method(realm, &number_function, "isFinite")?;
    assert!(matches!(
        heap.snapshot(&heap.call(&is_finite, &undefined, &[heap.string("1".into())?], runtime)?)?,
        ValueSnapshot::Boolean(false)
    ));
    let boxed_number = heap.opaque(OpaqueKind::BoxedNumber, CapabilityId(970), 970)?;
    // BoxedNumber is Object, so predicates must return false without coercion
    // or even trying to resolve this deliberately unregistered capability.
    assert!(matches!(
        heap.snapshot(&heap.call(&is_finite, &undefined, &[boxed_number.clone()], runtime)?)?,
        ValueSnapshot::Boolean(false)
    ));
    assert!(matches!(
        heap.call(&number_function, &undefined, &[boxed_number], runtime),
        Err(ValueError::MissingCapability(CapabilityId(970)))
    ));
    let now = method(realm, &realm.globals.date, "now")?;
    let before = milliseconds();
    let current = heap.call(&now, &realm.globals.date, &[], runtime)?;
    let after = milliseconds();
    assert!((before..=after).contains(&number(realm, &current)?));
    receipts.push(current);
    // Fixed Bun 1.4.0 Date.parse: date-only ISO is UTC independently of TZ.
    let parsed = heap.call(
        &method(realm, &realm.globals.date, "parse")?,
        &undefined,
        &[heap.string("2026-01-01".into())?],
        runtime,
    )?;
    assert_eq!(number(realm, &parsed)?, 1_767_225_600_000.0);

    struct Local<'a> {
        base: &'a dyn ValueRuntime,
        receiver: ValueHandle,
        result: ValueHandle,
        capability: CapabilityId,
        calls: AtomicUsize,
    }
    impl ValueRuntime for Local<'_> {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply> {
            self.base.resolve(request)
        }
        fn resolve_call(
            &self,
            capability: CapabilityId,
            receiver: &ValueHandle,
            args: &[ValueHandle],
            effective: &dyn ValueRuntime,
        ) -> Result<RuntimeReply> {
            if capability != self.capability {
                return self.base.resolve_call(capability, receiver, args, effective);
            }
            assert!(receiver.same_identity(&self.receiver));
            assert!(matches!(receiver.heap().snapshot(&args[0])?,ValueSnapshot::String(s) if s.equals_ascii("number")));
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(RuntimeReply::Value(self.result.clone()))
        }
    }
    let input = heap.object()?;
    let capability = CapabilityId(971);
    heap.define_own_data(&input, heap.to_primitive_key(), heap.function(capability)?, runtime)?;
    let local = Local {
        base: runtime,
        receiver: input.clone(),
        result: heap.number(42.0)?,
        capability,
        calls: AtomicUsize::new(0),
    };
    assert_eq!(number(realm, &heap.call(&number_function, &undefined, &[input.clone()], &local)?)?, 42.0);
    assert_eq!(local.calls.load(Ordering::Relaxed), 1);
    assert!(
        matches!(heap.call(&number_function,&undefined,&[input],runtime),Err(ValueError::MissingCapability(c)) if c==capability)
    );
    receipts.extend(observe_native_bun(realm)?);
    receipts.extend(observe_native_construct_map_hash(realm)?);
    receipts.extend(date::observe(realm)?);
    receipts.extend(async_context::observe(realm)?);
    receipts.extend(async_promise::observe(realm)?);
    receipts.extend(logger_time::observe(realm)?);
    receipts.extend(read_continuation::observe(realm)?);
    Ok(receipts)
}

/// Fixed same-revision Bun oracle, added to the existing realm coarse family.
/// The fixture is observation data, never a production Value graph substitute.
fn observe_native_bun(realm: &Arc<NativeRealm>) -> Result<Vec<ValueHandle>> {
    #[derive(serde::Deserialize)]
    struct Oracle {
        fixed_omp: String,
        bun_version: String,
        bun_revision: String,
        oracle_sha256: String,
        rows: Vec<Row>,
    }
    #[derive(serde::Deserialize)]
    struct Row {
        label: String,
        kind: String,
        input_units: Option<Vec<u16>>,
        ok: bool,
        output_units: Option<Vec<u16>>,
        original_marker: Option<bool>,
        error_name: Option<String>,
        error_message: Option<String>,
        effects: Vec<String>,
    }
    let oracle: Oracle = serde_json::from_str(include_str!("bun-oracle.json")).unwrap();
    assert_eq!(oracle.fixed_omp, "596f2da7101178214aa27a753529d15e6b7ad91d");
    assert_eq!(oracle.bun_version, "1.4.0");
    assert_eq!(oracle.bun_revision, "34cbb9a40b4bd1bd767d134a7065e66c2432a676");
    assert_eq!(oracle.oracle_sha256, "e2f86eb16f5a5a026fe12be1e7e989ae0e149b41da2f4c22cbdcce9688ebfef7");
    assert_eq!(oracle.rows.len(), 185);
    let heap = realm.values.heap();
    let runtime = realm.values.runtime();
    let bun = global(realm, "Bun")?;
    assert!(bun.same_identity(&realm.globals.bun));
    let strip = method(realm, &bun, "stripANSI")?;
    assert!(matches!(
        heap.own_descriptor(&bun, &"stripANSI".into(), runtime)?,
        Some(PropertyDescriptor::Data { writable: true, enumerable: true, configurable: false, .. })
    ));
    assert!(
        matches!(heap.snapshot(&method(realm, &strip, "name")?)?, ValueSnapshot::String(s) if s.equals_ascii("stripANSI"))
    );
    assert_eq!(number(realm, &method(realm, &strip, "length")?)?, 1.0);
    assert!(heap.own_descriptor(&strip, &"prototype".into(), runtime)?.is_none());
    assert!(matches!(heap.construct_compiled_function(&strip, &[], runtime), Err(ValueError::TypeError(_))));
    let undefined = heap.undefined()?;
    static GET_PRIMITIVE: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "controlled original Bun coercion Get/effect receipt",
        name: "get toPrimitive",
        length: 0,
        form: CompiledFunctionForm::Builtin,
        capture_count: 4,
        body: |heap, captures, receiver, _, runtime| {
            assert!(receiver.same_identity(&captures[3]));
            heap.array_push(&captures[0], &[heap.string("Get:Symbol.toPrimitive".into())?], runtime)?;
            if matches!(heap.snapshot(&captures[2])?, ValueSnapshot::Boolean(true)) {
                Err(ValueError::Thrown(captures[1].clone()))
            } else {
                Ok(captures[1].clone())
            }
        },
    };
    static CALL_PRIMITIVE: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "controlled original Bun coercion Call/string hint/effect receipt",
        name: "toPrimitive",
        length: 1,
        form: CompiledFunctionForm::Builtin,
        capture_count: 4,
        body: |heap, captures, receiver, arguments, runtime| {
            assert!(receiver.same_identity(&captures[3]));
            assert!(matches!(heap.snapshot(&arguments[0])?, ValueSnapshot::String(s) if s.equals_ascii("string")));
            heap.array_push(&captures[0], &[heap.string("Call:Symbol.toPrimitive:string".into())?], runtime)?;
            if matches!(heap.snapshot(&captures[2])?, ValueSnapshot::Boolean(true)) {
                Err(ValueError::Thrown(captures[1].clone()))
            } else {
                Ok(captures[1].clone())
            }
        },
    };
    struct Boxed<'a> {
        base: &'a dyn ValueRuntime,
        target: ValueHandle,
        primitive: ValueHandle,
        calls: AtomicUsize,
    }
    impl ValueRuntime for Boxed<'_> {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply> {
            match request {
                RuntimeRequest::Opaque {
                    capability: CapabilityId(974),
                    target,
                    receiver,
                    operation: OpaqueOperation::ToPrimitive { string_hint },
                } => {
                    assert!(string_hint);
                    assert!(target.same_identity(&self.target) && receiver.same_identity(&self.target));
                    self.calls.fetch_add(1, Ordering::Relaxed);
                    Ok(RuntimeReply::Value(self.primitive.clone()))
                }
                other => self.base.resolve(other),
            }
        }
    }
    let boxed_target = heap.opaque(OpaqueKind::BoxedString, CapabilityId(974), 974)?;
    let boxed = Boxed {
        base: runtime,
        target: boxed_target.clone(),
        primitive: heap.string("\x1b[31mA".into())?,
        calls: AtomicUsize::new(0),
    };
    let mut receipts = Vec::new();
    for row in oracle.rows {
        let effects = heap.array(vec![])?;
        let marker = heap.object()?;
        let input = match row.kind.as_str() {
            "string" => Some(heap.string(WireString::from_units(row.input_units.unwrap()))?),
            "missing" => None,
            "undefined" => Some(undefined.clone()),
            "null" => Some(heap.null()?),
            "false" | "true" => Some(heap.boolean(row.kind == "true")?),
            "zero" => Some(heap.number(0.0)?),
            "negative_zero" => Some(heap.number(-0.0)?),
            "number" => Some(heap.number(12.5)?),
            "nan" => Some(heap.number(f64::NAN)?),
            "infinity" => Some(heap.number(f64::INFINITY)?),
            "bigint" => Some(heap.bigint_decimal("123")?),
            "symbol" => Some(heap.symbol(Some("s".into()))?),
            "empty_object" => Some(heap.object()?),
            "array" => Some(heap.array(vec![heap.string("\x1b[31mA".into())?, heap.string("B".into())?])?),
            "boxed_string" => Some(boxed_target.clone()),
            "coerce_string" | "coerce_symbol" | "coerce_throw" | "coerce_get_throw" => {
                let object = heap.object_with_prototype(None)?;
                let result = match row.kind.as_str() {
                    "coerce_string" => heap.string("\x1b[31mA\x1b[0m".into())?,
                    "coerce_symbol" => heap.symbol(Some("coerced".into()))?,
                    _ => marker.clone(),
                };
                let call = heap.fresh_compiled_function(
                    &CALL_PRIMITIVE,
                    &[effects.clone(), result, heap.boolean(row.kind == "coerce_throw")?, object.clone()],
                )?;
                let getter = heap.fresh_compiled_function(
                    &GET_PRIMITIVE,
                    &[
                        effects.clone(),
                        if row.kind == "coerce_get_throw" { marker.clone() } else { call },
                        heap.boolean(row.kind == "coerce_get_throw")?,
                        object.clone(),
                    ],
                )?;
                let ValueSnapshot::Function(getter) = heap.snapshot(&getter)? else {
                    return Err(ValueError::InvalidNode);
                };
                heap.define_own(
                    &object,
                    heap.to_primitive_key(),
                    PropertyDescriptor::Accessor {
                        get: Some(getter),
                        set: None,
                        enumerable: false,
                        configurable: true,
                    },
                    runtime,
                )?;
                Some(object)
            }
            unknown => panic!("Unknown frozen Bun case {unknown}"),
        };
        let arguments = input.into_iter().collect::<Vec<_>>();
        let effective: &dyn ValueRuntime = if row.kind == "boxed_string" { &boxed } else { runtime };
        let result = heap.call(&strip, &heap.null()?, &arguments, effective);
        let receipt = if row.ok {
            let output = result.unwrap_or_else(|error| panic!("Bun {}: {error:?}", row.label));
            assert!(
                matches!(heap.snapshot(&output)?, ValueSnapshot::String(ref s) if Some(s.units()) == row.output_units.as_deref()),
                "Bun {}",
                row.label
            );
            output
        } else {
            let original = thrown(result)?;
            if row.original_marker == Some(true) {
                assert!(original.same_identity(&marker), "Bun {} original thrown handle", row.label);
            } else {
                assert!(heap.instance_of(&original, &realm.globals.errors.type_error, runtime)?, "Bun {}", row.label);
                for (key, expected) in [("name", row.error_name), ("message", row.error_message)] {
                    let value = method(realm, &original, key)?;
                    let expected = expected.map(WireString::from);
                    assert!(
                        matches!(heap.snapshot(&value)?, ValueSnapshot::String(ref s) if Some(s) == expected.as_ref()),
                        "Bun {} {key}",
                        row.label
                    );
                }
            }
            original
        };
        let actual_effects = (0..heap.array_length(&effects, runtime)?)
            .map(|index| {
                let value = heap.get(&effects, &WireString::from(index.to_string()).into(), runtime)?;
                match heap.snapshot(&value)? {
                    ValueSnapshot::String(s) => Ok(s.to_utf8().unwrap()),
                    _ => Err(ValueError::InvalidNode),
                }
            })
            .collect::<Result<Vec<_>>>()?;
        assert_eq!(actual_effects, row.effects, "Bun {} effects", row.label);
        receipts.push(receipt);
    }
    assert_eq!(boxed.calls.load(Ordering::Relaxed), 1);
    // The current fixed sanitizer calls this actual Bun only for ESC input,
    // then applies its own control removal and lone-surrogate policy.
    let sanitizer = ara_agent::ActualToolResultSanitizer::new(bun.clone(), &realm.values).unwrap();
    let mut text = "\x1b[31mA\x1b[0m\0\r\t\n".encode_utf16().collect::<Vec<_>>();
    text.push(0xd800);
    let sanitized = sanitizer.sanitize(&heap.string(WireString::from_units(text))?, &realm.values).unwrap();
    assert!(matches!(heap.snapshot(&sanitized)?, ValueSnapshot::String(s) if s.equals_ascii("A\t\n")));
    receipts.push(sanitized);
    static CURRENT_STRIP: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "controlled same-realm sanitizer dynamic Bun.stripANSI Call receipt",
        name: "stripANSI",
        length: 1,
        form: CompiledFunctionForm::Builtin,
        capture_count: 3,
        body: |heap, captures, receiver, arguments, runtime| {
            assert!(receiver.same_identity(&captures[2]));
            heap.array_push(&captures[0], &[heap.boolean(true)?], runtime)?;
            heap.call(&captures[1], receiver, arguments, runtime)
        },
    };
    let calls = heap.array(vec![])?;
    let replacement = heap.fresh_compiled_function(&CURRENT_STRIP, &[calls.clone(), strip.clone(), bun.clone()])?;
    heap.assign(&bun, "stripANSI".into(), replacement, runtime)?;
    let dynamic = sanitizer.sanitize(&heap.string("\x1b[31mB\x1b[0m".into())?, &realm.values).unwrap();
    assert!(matches!(heap.snapshot(&dynamic)?, ValueSnapshot::String(s) if s.equals_ascii("B")));
    assert_eq!(heap.array_length(&calls, runtime)?, 1);
    heap.assign(&bun, "stripANSI".into(), strip, runtime)?;
    receipts.push(dynamic);
    // Factory admission uses the installed original object and never probes
    // stripANSI eagerly. No formal factory or logger is installed here.
    let source = BuiltinToolSource::new(&realm.values, ToolContext::new(std::env::temp_dir()), &[]).unwrap();
    let bindings = |bun| ara_cli::native_factory::NativeFactoryImports {
        source: source.clone(),
        string_function: realm.globals.string.clone(),
        bun,
        original_logger: None,
    };
    assert!(
        ara_cli::native_factory::PreparedNativeSession::from_complete_bindings(realm.clone(), bindings(heap.object()?))
            .is_err()
    );
    let foreign = NativeRealm::prepare()?;
    assert!(
        ara_cli::native_factory::PreparedNativeSession::from_complete_bindings(
            realm.clone(),
            bindings(foreign.globals.bun.clone())
        )
        .is_err()
    );
    let complete =
        ara_cli::native_factory::PreparedNativeSession::from_complete_bindings(realm.clone(), bindings(bun)).unwrap();
    assert!(complete.require_tool_hooks().is_ok());
    assert!(complete.require_logger().is_err());
    assert!(Arc::ptr_eq(complete.values(), &realm.values));
    eprintln!("Bun same-revision observation: 185 cases; original coercion/throws and actual sanitizer connected");
    Ok(receipts)
}

/// Selected construction and frozen hash oracle inside the existing realm gate.
/// Opaque/scoped faults here are controlled observations, not Host product acceptance.
fn observe_native_construct_map_hash(realm: &Arc<NativeRealm>) -> Result<Vec<ValueHandle>> {
    let heap = realm.values.heap();
    let runtime = realm.values.runtime();
    let undefined = heap.undefined()?;
    let map_constructor = global(realm, "Map")?;
    let map_prototype = heap.map_intrinsic_prototype();
    assert!(map_constructor.same_identity(&realm.globals.map));
    assert!(map_constructor.same_identity(&heap.map_constructor()));
    assert!(method(realm, &map_constructor, "prototype")?.same_identity(&map_prototype));
    assert!(method(realm, &map_prototype, "constructor")?.same_identity(&map_constructor));
    assert!(matches!(
        heap.own_descriptor(&map_constructor, &"prototype".into(), runtime)?,
        Some(PropertyDescriptor::Data { writable: false, enumerable: false, configurable: false, .. })
    ));
    assert!(heap.is_constructor(&map_constructor, runtime)?);
    // Core correctly distinguishes Call from Construct. This raw TypeError
    // does not prove the still-open Native Map Call original-realm Error seam.
    assert!(matches!(heap.call(&map_constructor, &undefined, &[], runtime), Err(ValueError::TypeError(_))));
    let map = heap.construct(&map_constructor, &[], runtime)?;
    assert!(heap.prototype(&map, runtime)?.unwrap().same_identity(&map_prototype));
    assert_eq!(number(realm, &method(realm, &map, "size")?)?, 0.0);
    let set = method(realm, &map, "set")?;
    let get = method(realm, &map, "get")?;
    let clear = method(realm, &map, "clear")?;
    let first = heap.string("first".into())?;
    let second = heap.string("second".into())?;
    assert!(heap.call(&set, &map, &[heap.number(-0.0)?, first.clone()], runtime)?.same_identity(&map));
    assert!(heap.call(&get, &map, &[heap.number(0.0)?], runtime)?.same_identity(&first));
    heap.call(&set, &map, &[heap.number(f64::NAN)?, second.clone()], runtime)?;
    assert!(heap.call(&get, &map, &[heap.number(f64::NAN)?], runtime)?.same_identity(&second));
    let keys = heap.call(&method(realm, &map, "keys")?, &map, &[], runtime)?;
    let next = method(realm, &keys, "next")?;
    let first_key = heap.call(&next, &keys, &[], runtime)?;
    assert!(heap.object_is(&method(realm, &first_key, "value")?, &heap.number(0.0)?)?);
    let second_key = heap.call(&next, &keys, &[], runtime)?;
    assert!(number(realm, &method(realm, &second_key, "value")?)?.is_nan());
    let last_key = heap.call(&next, &keys, &[], runtime)?;
    assert!(matches!(heap.snapshot(&method(realm, &last_key, "done")?)?, ValueSnapshot::Boolean(true)));
    heap.call(&clear, &map, &[], runtime)?;
    assert_eq!(number(realm, &method(realm, &map, "size")?)?, 0.0);
    for empty in [undefined.clone(), heap.null()?] {
        let output = heap.construct(&map_constructor, &[empty], runtime)?;
        assert!(heap.prototype(&output, runtime)?.unwrap().same_identity(&map_prototype));
    }
    // The selected intrinsic Construct uses the original iterable algorithm
    // and ignores extra arguments, matching the global consumed by callers.
    for arguments in [vec![heap.array(vec![])?], vec![undefined.clone(), undefined.clone()]] {
        let output = heap.construct(&map_constructor, &arguments, runtime)?;
        assert!(heap.prototype(&output, runtime)?.unwrap().same_identity(&map_prototype));
        assert_eq!(number(realm, &method(realm, &output, "size")?)?, 0.0);
    }

    static ORDINARY: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "controlled neutral Ordinary constructor result override and captures",
        name: "Ordinary",
        length: 1,
        form: CompiledFunctionForm::Ordinary,
        capture_count: 1,
        body: |heap, captures, receiver, arguments, effective| {
            heap.define_own_data(receiver, "value".into(), captures[0].clone(), effective)?;
            Ok(arguments.first().cloned().unwrap_or(heap.undefined()?))
        },
    };
    static ARROW: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "controlled neutral Arrow nonconstructor",
        name: "Arrow",
        length: 0,
        form: CompiledFunctionForm::Arrow,
        capture_count: 0,
        body: |heap, _, _, _, _| heap.undefined(),
    };
    static BUILTIN: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "controlled neutral Builtin nonconstructor",
        name: "Builtin",
        length: 0,
        form: CompiledFunctionForm::Builtin,
        capture_count: 0,
        body: |heap, _, _, _, _| heap.undefined(),
    };
    fn native_constructor(
        heap: &Arc<ara_rpc::value_heap::ValueHeap>,
        _: &[ValueHandle],
        receiver: &ValueHandle,
        _: &[ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> Result<ValueHandle> {
        heap.define_own_data(receiver, "constructed".into(), heap.boolean(true)?, effective)?;
        heap.undefined()
    }
    static NATIVE: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "controlled separate Native constructor body",
        name: "Native",
        length: 0,
        form: CompiledFunctionForm::NativeConstructor(native_constructor),
        capture_count: 0,
        body: |heap, _, _, _, _| heap.number(7.0),
    };
    let ordinary = heap.fresh_compiled_function(&ORDINARY, &[first.clone()])?;
    assert!(heap.is_constructor(&ordinary, runtime)?);
    let result = heap.construct(&ordinary, &[heap.number(1.0)?], runtime)?;
    assert!(heap.prototype(&result, runtime)?.unwrap().same_identity(&method(realm, &ordinary, "prototype")?));
    assert!(method(realm, &result, "value")?.same_identity(&first));
    let returned_object = heap.object()?;
    assert!(heap.construct(&ordinary, &[returned_object.clone()], runtime)?.same_identity(&returned_object));
    for spec in [&ARROW, &BUILTIN] {
        let function = heap.fresh_compiled_function(spec, &[])?;
        assert!(!heap.is_constructor(&function, runtime)?);
        assert!(matches!(heap.construct(&function, &[], runtime), Err(ValueError::TypeError(_))));
    }
    assert!(!heap.is_constructor(&undefined, runtime)?);
    let native = heap.fresh_compiled_function(&NATIVE, &[])?;
    assert!(heap.is_constructor(&native, runtime)?);
    assert_eq!(number(realm, &heap.call(&native, &undefined, &[], runtime)?)?, 7.0);
    let native_result = heap.construct(&native, &[], runtime)?;
    assert!(matches!(heap.snapshot(&method(realm, &native_result, "constructed")?)?, ValueSnapshot::Boolean(true)));
    // The selected binding is observed at the graph call site; a replacement
    // constructor must be used as selected, never substituted with intrinsic Map.
    heap.assign(&realm.globals.environment, "Map".into(), native.clone(), runtime)?;
    let replacement = heap.construct(&global(realm, "Map")?, &[], runtime)?;
    assert!(matches!(heap.snapshot(&method(realm, &replacement, "constructed")?)?, ValueSnapshot::Boolean(true)));
    heap.assign(&realm.globals.environment, "Map".into(), map_constructor.clone(), runtime)?;

    static CONSTRUCT_WITH_CALLBACK: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "controlled constructor through OperationRuntime and ScopedRuntime",
        name: "constructWithCallback",
        length: 1,
        form: CompiledFunctionForm::Builtin,
        capture_count: 1,
        body: |heap, captures, _, arguments, effective| heap.construct(&captures[0], arguments, effective),
    };
    struct SelectedHost<'a> {
        realm: &'a NativeRealm,
        target: &'a ValueHandle,
        expected_scope: ara_cli::native_realm::context::NativeScope,
        probes: AtomicUsize,
        constructs: AtomicUsize,
    }
    impl ValueRuntime for SelectedHost<'_> {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply> {
            self.realm.values.runtime().resolve(request)
        }
        fn resolve_effective(&self, request: RuntimeRequest<'_>, effective: &dyn ValueRuntime) -> Result<RuntimeReply> {
            self.realm.values.runtime().resolve_effective(request, effective)
        }
        fn resolve_call(
            &self,
            capability: CapabilityId,
            receiver: &ValueHandle,
            arguments: &[ValueHandle],
            effective: &dyn ValueRuntime,
        ) -> Result<RuntimeReply> {
            self.realm.values.runtime().resolve_call(capability, receiver, arguments, effective)
        }
        fn resolve_local_call(
            &self,
            token: u64,
            receiver: &ValueHandle,
            arguments: &[ValueHandle],
            effective: &dyn ValueRuntime,
        ) -> Result<RuntimeReply> {
            self.realm.values.runtime().resolve_local_call(token, receiver, arguments, effective)
        }
        fn resolve_is_constructor(
            &self,
            capability: CapabilityId,
            constructor: &ValueHandle,
            effective: &dyn ValueRuntime,
        ) -> Result<bool> {
            assert_eq!(capability, CapabilityId(976));
            assert!(constructor.same_identity(self.target));
            assert_eq!(self.realm.context().capture_scope(constructor.heap(), effective)?, Some(self.expected_scope));
            self.probes.fetch_add(1, Ordering::Relaxed);
            Ok(true)
        }
        fn resolve_construct(
            &self,
            capability: CapabilityId,
            constructor: &ValueHandle,
            arguments: &[ValueHandle],
            effective: &dyn ValueRuntime,
        ) -> Result<ValueHandle> {
            assert_eq!(capability, CapabilityId(976));
            assert!(constructor.same_identity(self.target));
            assert_eq!(self.realm.context().capture_scope(constructor.heap(), effective)?, Some(self.expected_scope));
            self.constructs.fetch_add(1, Ordering::Relaxed);
            assert_eq!(arguments.len(), 1);
            constructor.heap().call(&arguments[0], &constructor.heap().undefined()?, &[], effective)
        }
    }
    let opaque = heap.opaque(OpaqueKind::Function, CapabilityId(976), 976)?;
    assert!(matches!(heap.is_constructor(&opaque, runtime), Err(ValueError::UnsupportedOperation(_))));
    assert!(matches!(heap.construct(&opaque, &[], runtime), Err(ValueError::UnsupportedOperation(_))));
    heap.assign(&realm.globals.environment, "Map".into(), opaque.clone(), runtime)?;
    assert!(matches!(heap.construct(&global(realm, "Map")?, &[], runtime), Err(ValueError::UnsupportedOperation(_))));
    heap.assign(&realm.globals.environment, "Map".into(), map_constructor.clone(), runtime)?;
    let host_run = realm.begin_context_run()?;
    let selected = SelectedHost {
        realm,
        target: &opaque,
        expected_scope: host_run.root_scope(),
        probes: AtomicUsize::new(0),
        constructs: AtomicUsize::new(0),
    };
    let scoped = host_run.scoped(host_run.root_scope(), &selected)?;
    let adapter = heap.fresh_compiled_function(&CONSTRUCT_WITH_CALLBACK, &[opaque.clone()])?;
    let selected_result = heap.object()?;
    let mut calls = 0;
    let mut callback = |_: &ValueHandle, _: &[ValueHandle], effective: &dyn ValueRuntime| {
        assert_eq!(realm.context().capture_scope(heap, effective)?, Some(host_run.root_scope()));
        calls += 1;
        Ok(selected_result.clone())
    };
    let output = heap.call_with_local_callback(&adapter, &undefined, &mut callback, &[], &scoped)?;
    assert!(output.same_identity(&selected_result));
    assert_eq!(calls, 1);
    assert_eq!(selected.probes.load(Ordering::Relaxed), 1);
    assert_eq!(selected.constructs.load(Ordering::Relaxed), 1);
    drop(scoped);
    drop(host_run);

    struct ConstructorReply {
        allowed: bool,
        reply: Result<ValueHandle>,
    }
    impl ValueRuntime for ConstructorReply {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply> {
            Err(ValueError::MissingCapability(request.capability()))
        }
        fn resolve_is_constructor(&self, _: CapabilityId, _: &ValueHandle, _: &dyn ValueRuntime) -> Result<bool> {
            Ok(self.allowed)
        }
        fn resolve_construct(
            &self,
            _: CapabilityId,
            _: &ValueHandle,
            _: &[ValueHandle],
            _: &dyn ValueRuntime,
        ) -> Result<ValueHandle> {
            self.reply.clone()
        }
    }
    let host_function = heap.function(CapabilityId(977))?;
    assert!(matches!(heap.is_constructor(&host_function, runtime), Err(ValueError::UnsupportedOperation(_))));
    let nonconstructor = ConstructorReply { allowed: false, reply: Ok(selected_result.clone()) };
    assert!(!heap.is_constructor(&host_function, &nonconstructor)?);
    assert!(matches!(heap.construct(&host_function, &[], &nonconstructor), Err(ValueError::TypeError(_))));
    let primitive_reply = ConstructorReply { allowed: true, reply: Ok(undefined.clone()) };
    assert!(matches!(heap.construct(&host_function, &[], &primitive_reply), Err(ValueError::WrongRuntimeReply(_))));
    let foreign = NativeRealm::prepare()?;
    let foreign_value = foreign.values.heap().object()?;
    for reply in [Ok(foreign_value.clone()), Err(ValueError::Thrown(foreign_value.clone()))] {
        let replies = ConstructorReply { allowed: true, reply };
        assert!(matches!(heap.construct(&host_function, &[], &replies), Err(ValueError::ForeignHeap)));
    }
    assert!(matches!(
        heap.construct(&map_constructor, &[foreign_value.clone()], runtime),
        Err(ValueError::ForeignHeap)
    ));
    assert!(matches!(heap.is_constructor(&foreign_value, runtime), Err(ValueError::ForeignHeap)));
    let marker = heap.object()?;
    let original_throw = ConstructorReply { allowed: true, reply: Err(ValueError::Thrown(marker.clone())) };
    assert!(thrown(heap.construct(&host_function, &[], &original_throw))?.same_identity(&marker));
    for fault in [ValueError::ExpiredCapability(CapabilityId(977)), ValueError::Allocation] {
        let replies = ConstructorReply { allowed: true, reply: Err(fault) };
        let error = heap.construct(&host_function, &[], &replies).unwrap_err();
        assert!(matches!(error, ValueError::ExpiredCapability(_) | ValueError::Allocation));
        assert!(matches!(
            realm.globals.errors.create_at_native_origin(error, runtime),
            ValueError::ExpiredCapability(_) | ValueError::Allocation
        ));
    }

    let hash = method(realm, &realm.globals.bun, "hash")?;
    let xxhash = method(realm, &hash, "xxHash64")?;
    assert!(matches!(heap.snapshot(&hash)?, ValueSnapshot::Function(_)));
    assert!(
        matches!(heap.snapshot(&method(realm, &hash, "name")?)?, ValueSnapshot::String(s) if s.equals_ascii("hash"))
    );
    assert_eq!(number(realm, &method(realm, &hash, "length")?)?, 1.0);
    assert!(
        matches!(heap.snapshot(&method(realm, &xxhash, "name")?)?, ValueSnapshot::String(s) if s.equals_ascii("xxHash64"))
    );
    assert_eq!(number(realm, &method(realm, &xxhash, "length")?)?, 1.0);
    assert!(matches!(
        heap.own_descriptor(&hash, &"xxHash64".into(), runtime)?,
        Some(PropertyDescriptor::Data { writable: true, enumerable: true, configurable: true, .. })
    ));
    assert!(!heap.is_constructor(&hash, runtime)?);
    assert!(!heap.is_constructor(&xxhash, runtime)?);
    assert!(matches!(
        heap.call(&hash, &undefined, &[heap.string("x".into())?], runtime),
        Err(ValueError::UnsupportedOperation(_))
    ));
    for name in [
        "wyhash",
        "adler32",
        "crc32",
        "cityHash32",
        "cityHash64",
        "xxHash32",
        "xxHash3",
        "murmur32v2",
        "murmur32v3",
        "murmur64v2",
        "rapidhash",
    ] {
        let unselected = method(realm, &hash, name)?;
        assert!(matches!(
            heap.call(&unselected, &hash, &[heap.string("x".into())?], runtime),
            Err(ValueError::UnsupportedOperation(_))
        ));
    }
    // Exactly the eight actual frozen Bun observations (stdout SHA d6721020...).
    let rows: &[(&[u16], &str)] = &[
        (&[], "17241709254077376921"),
        (&[120], "6665539201184043299"),
        (&[0, 65, 0], "12557272373683741796"),
        (&[233], "1717938401253289848"),
        (&[20013], "9984719041498862281"),
        (&[55357, 56832], "10386911163046198144"),
        (&[55296], "4393271577342690558"),
        (&[56320], "4393271577342690558"),
    ];
    let mut receipts = vec![map, result, returned_object, native_result, selected_result, marker];
    for (units, decimal) in rows {
        let output = heap.call(&xxhash, &hash, &[heap.string(WireString::from_units(units.to_vec()))?], runtime)?;
        assert!(matches!(heap.snapshot(&output)?, ValueSnapshot::BigInt(value) if value.as_str() == *decimal));
        receipts.push(output);
    }
    for arguments in [vec![], vec![heap.number(1.0)?], vec![heap.string("x".into())?, heap.number(0.0)?]] {
        assert!(matches!(heap.call(&xxhash, &hash, &arguments, runtime), Err(ValueError::UnsupportedOperation(_))));
    }
    assert!(matches!(heap.call(&xxhash, &hash, &[foreign_value], runtime), Err(ValueError::ForeignHeap)));
    eprintln!(
        "Native neutral Construct/Map identity/effective Host and frozen xxHash64 8-row observation family executed; full newTarget, iterable, general opaque and native Map Call Error remain open"
    );
    Ok(receipts)
}

/// Actual source/hook/default-runtime cancellation + owner-expiry family.
/// Source has no tool kernels here; this performs no OS effects.
pub fn observe_native_realm_signal_registration(cwd: PathBuf) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let realm = NativeRealm::prepare()?;
    let source = BuiltinToolSource::new(&realm.values, ToolContext::new(cwd.clone()), &[])?;
    let hooks =
        NativeBuiltinToolHooks::new(source.clone(), Some(realm.error_constructor()), None, None, &realm.values)?;
    realm.bind_builtin_source(source.clone(), &hooks)?;
    realm.bind_builtin_source(source.clone(), &hooks)?;
    let wrong = BuiltinToolSource::new(&realm.values, ToolContext::new(cwd), &[])?;
    assert!(realm.bind_builtin_source(wrong.clone(), &hooks).is_err());
    let wrong_hooks =
        NativeBuiltinToolHooks::new(wrong.clone(), Some(realm.error_constructor()), None, None, &realm.values)?;
    assert!(realm.bind_builtin_source(wrong, &wrong_hooks).is_err());
    let other = NativeRealm::prepare()?;
    assert!(realm.check_values(&other.values).is_err());
    let heap = realm.values.heap();
    let cancel = CancellationToken::new();
    let context = ActualToolExecutionContext {
        batch_id: "native-realm".into(),
        index: 0,
        total: 1,
        tool_calls: heap.array(vec![])?,
        provider_metadata: heap.object()?,
        steering_signal: cancel.clone(),
    };
    let input = hooks.tool_context(&context, &realm.values)?;
    let call = heap.get(&input, &"toolCall".into(), realm.values.runtime())?;
    let signal = heap.get(&call, &"steeringSignal".into(), realm.values.runtime())?;
    assert!(matches!(
        heap.snapshot(&heap.get(&signal, &"aborted".into(), realm.values.runtime())?)?,
        ValueSnapshot::Boolean(false)
    ));
    cancel.cancel();
    assert!(matches!(
        heap.snapshot(&heap.get(&signal, &"aborted".into(), realm.values.runtime())?)?,
        ValueSnapshot::Boolean(true)
    ));
    assert!(matches!(
        heap.get(&signal, &"reason".into(), realm.values.runtime()),
        Err(ValueError::UnsupportedOperation(_))
    ));
    let unknown = heap.opaque(OpaqueKind::Object, CapabilityId(972), 972)?;
    assert!(matches!(
        heap.get(&unknown, &"aborted".into(), realm.values.runtime()),
        Err(ValueError::MissingCapability(CapabilityId(972)))
    ));
    let retained_runtime = realm.runtime.clone();
    let weak_values = Arc::downgrade(&realm.values);
    let heap = heap.clone();
    drop(context);
    drop(input);
    drop(call);
    drop(realm);
    assert!(weak_values.upgrade().is_none(), "runtime/source must not retain values/factory");
    assert!(matches!(
        heap.get(&signal, &"aborted".into(), retained_runtime.as_ref()),
        Err(ValueError::ExpiredCapability(_))
    ));
    drop(hooks);
    drop(source);
    assert!(matches!(
        heap.get(&signal, &"aborted".into(), retained_runtime.as_ref()),
        Err(ValueError::MissingCapability(_))
    ));
    Ok(())
}

/// Bounded concurrent readiness observation within the same coarse entrance.
/// Every successful/idempotent return must immediately use the real resolver.
pub fn observe_native_realm_concurrent_binding(cwd: PathBuf) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let realm = NativeRealm::prepare()?;
    let source = BuiltinToolSource::new(&realm.values, ToolContext::new(cwd), &[])?;
    let hooks =
        NativeBuiltinToolHooks::new(source.clone(), Some(realm.error_constructor()), None, None, &realm.values)?;
    let heap = realm.values.heap();
    let context = ActualToolExecutionContext {
        batch_id: "concurrent-native-realm".into(),
        index: 0,
        total: 1,
        tool_calls: heap.array(vec![])?,
        provider_metadata: heap.object()?,
        steering_signal: CancellationToken::new(),
    };
    let input = hooks.tool_context(&context, &realm.values)?;
    let call = heap.get(&input, &"toolCall".into(), realm.values.runtime())?;
    let signal = heap.get(&call, &"steeringSignal".into(), realm.values.runtime())?;
    assert!(matches!(
        heap.get(&signal, &"aborted".into(), realm.values.runtime()),
        Err(ValueError::MissingCapability(_))
    ));
    let barrier = Arc::new(Barrier::new(16));
    let successes = Arc::new(AtomicUsize::new(0));
    std::thread::scope(|scope| {
        let mut workers = Vec::new();
        for _ in 0..16 {
            let realm = realm.clone();
            let source = source.clone();
            let hooks = hooks.clone();
            let signal = signal.clone();
            let barrier = barrier.clone();
            let successes = successes.clone();
            workers.push(scope.spawn(move || {
                barrier.wait();
                if realm.bind_builtin_source(source, &hooks).is_ok() {
                    let heap = realm.values.heap();
                    let output = heap
                        .get(&signal, &"aborted".into(), realm.values.runtime())
                        .expect("Every successful bind has a registry");
                    assert!(matches!(heap.snapshot(&output).unwrap(), ValueSnapshot::Boolean(false)));
                    successes.fetch_add(1, Ordering::Relaxed);
                }
            }));
        }
        for worker in workers {
            worker.join().unwrap();
        }
    });
    assert!(successes.load(Ordering::Relaxed) > 0);
    realm.bind_builtin_source(source, &hooks)?;
    assert!(matches!(
        heap.snapshot(&heap.get(&signal, &"aborted".into(), realm.values.runtime())?)?,
        ValueSnapshot::Boolean(false)
    ));
    Ok(())
}
