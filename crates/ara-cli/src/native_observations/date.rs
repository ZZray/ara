//! Additional Date family inside the existing realm coarse entrance.
//! Controlled timezone hosts compare actual compiled bodies with Root's
//! same-revision Bun UTC/+08 receipts. No new test attribute or logger shell.

use super::{Result, global, method, number, thrown};
use ara_cli::native_realm::{
    NativeRealm,
    date::{DateHost, DateLocalParts},
};
use ara_rpc::value_heap::{
    CompiledFunctionForm, CompiledFunctionSpec, PropertyDescriptor, StringifyMode, ValueError, ValueHandle, ValueHeap,
    ValueSnapshot,
};
use chrono::{Datelike, FixedOffset, TimeZone, Timelike, Utc};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

fn oracle_number(value: &serde_json::Value) -> f64 {
    value.as_f64().unwrap_or_else(|| match value.as_str().unwrap() {
        "NaN" => f64::NAN,
        "Infinity" => f64::INFINITY,
        "-Infinity" => f64::NEG_INFINITY,
        other => panic!("Unexpected frozen Date number {other}"),
    })
}
fn expected_number(realm: &NativeRealm, value: &ValueHandle, expected: &serde_json::Value) -> Result<()> {
    let actual = number(realm, value)?;
    let expected = oracle_number(expected);
    assert!(actual == expected || actual.is_nan() && expected.is_nan(), "Date {actual} != {expected}");
    Ok(())
}
fn fields(
    realm: &NativeRealm,
    date: &ValueHandle,
    names: &[serde_json::Value],
    expected: &[serde_json::Value],
) -> Result<()> {
    let heap = realm.values.heap();
    for (name, expected) in names.iter().zip(expected) {
        let getter = method(realm, date, name.as_str().unwrap())?;
        expected_number(realm, &heap.call(&getter, date, &[], realm.values.runtime())?, expected)?;
    }
    Ok(())
}
struct FixedHost {
    offset: FixedOffset,
    now_calls: Arc<AtomicUsize>,
}
impl DateHost for FixedHost {
    fn now_ms(&self) -> Result<f64> {
        self.now_calls.fetch_add(1, Ordering::Relaxed);
        Ok(1_704_067_200_123.0)
    }
    fn local_parts(&self, ms: i64) -> Result<DateLocalParts> {
        let date = chrono::DateTime::<Utc>::from_timestamp_millis(ms).unwrap().with_timezone(&self.offset);
        Ok(DateLocalParts {
            year: date.year(),
            month: date.month(),
            day: date.day(),
            hour: date.hour(),
            minute: date.minute(),
            second: date.second(),
            millisecond: date.timestamp_subsec_millis(),
            timezone_offset: -f64::from(self.offset.local_minus_utc()) / 60.0,
        })
    }
    fn local_day_to_utc(&self, current: DateLocalParts, day: i64) -> Result<f64> {
        let wall = current.wall_time_with_day(day)?;
        Ok(self.offset.from_local_datetime(&wall).single().unwrap().timestamp_millis() as f64)
    }
}

fn exact_text(realm: &NativeRealm, value: &ValueHandle, expected: &str) -> Result<()> {
    assert!(matches!(realm.values.heap().snapshot(value)?, ValueSnapshot::String(text) if text.equals_ascii(expected)));
    Ok(())
}
fn json_family(realm: &NativeRealm, date: &ValueHandle, prototype: &ValueHandle) -> Result<Vec<ValueHandle>> {
    let oracle: serde_json::Value = serde_json::from_str(include_str!("date-json-oracle.json")).unwrap();
    assert_eq!(oracle["bun_version"], "1.4.0");
    assert_eq!(oracle["bun_revision"], "34cbb9a40b4bd1bd767d134a7065e66c2432a676");
    let heap = realm.values.heap();
    let runtime = realm.values.runtime();
    let to_json = method(realm, prototype, "toJSON")?;
    let to_iso = method(realm, prototype, "toISOString")?;
    let mut receipts = Vec::new();
    for (name, function) in [("toJSON", &to_json), ("toISOString", &to_iso)] {
        let reference = &oracle["descriptors"][name];
        assert!(matches!(heap.own_descriptor(prototype, &name.into(), runtime)?, Some(PropertyDescriptor::Data {
            value, writable: true, enumerable: false, configurable: true,
        }) if value.same_identity(function)));
        exact_text(realm, &method(realm, function, "name")?, reference["name"].as_str().unwrap())?;
        assert_eq!(number(realm, &method(realm, function, "length")?)?, reference["length"].as_f64().unwrap());
    }
    for row in oracle["dates"].as_array().unwrap() {
        let instance =
            heap.construct_compiled_function(date, &[heap.number(oracle_number(&row["input"]))?], runtime)?;
        let iso = heap.call(&to_iso, &instance, &[], runtime);
        if row["iso"]["ok"].as_bool().unwrap() {
            let iso = iso?;
            exact_text(realm, &iso, row["iso"]["value"].as_str().unwrap())?;
            exact_text(realm, &heap.call(&to_json, &instance, &[], runtime)?, row["json"].as_str().unwrap())?;
            receipts.push(iso);
        } else {
            let error = thrown(iso)?;
            assert!(heap.instance_of(&error, &realm.globals.errors.range_error, runtime)?);
            exact_text(realm, &method(realm, &error, "message")?, row["iso"]["message"].as_str().unwrap())?;
            assert!(matches!(heap.snapshot(&heap.call(&to_json, &instance, &[], runtime)?)?, ValueSnapshot::Null));
            receipts.push(error);
        }
        let context = heap.object()?;
        heap.define_own_data(&context, "date".into(), instance.clone(), runtime)?;
        let serialized = heap.stringify(&context, StringifyMode::Plain, None, runtime)?.unwrap();
        assert!(serialized.equals_ascii(row["serialized"].as_str().unwrap()));
        receipts.extend([instance, context]);
    }
    static CONVERT: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "controlled Date.toJSON original number-hint conversion order/throw",
        name: "toPrimitive",
        length: 1,
        form: CompiledFunctionForm::Builtin,
        capture_count: 4,
        body: |heap, captures, _, arguments, runtime| {
            assert!(
                matches!(heap.snapshot(&arguments[0])?, ValueSnapshot::String(text) if text.equals_ascii("number"))
            );
            heap.array_push(&captures[0], &[heap.string("convert:number".into())?], runtime)?;
            if matches!(heap.snapshot(&captures[3])?, ValueSnapshot::Boolean(true)) {
                return Err(ValueError::Thrown(captures[2].clone()));
            }
            Ok(captures[1].clone())
        },
    };
    static CALL: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "controlled Date.toJSON dynamic ISO call receiver/arguments/throw",
        name: "toISOString",
        length: 0,
        form: CompiledFunctionForm::Builtin,
        capture_count: 4,
        body: |heap, captures, receiver, arguments, runtime| {
            assert!(receiver.same_identity(&captures[3]));
            assert!(arguments.is_empty());
            heap.array_push(&captures[0], &[heap.string("call:true:0".into())?], runtime)?;
            if matches!(heap.snapshot(&captures[2])?, ValueSnapshot::Boolean(true)) {
                return Err(ValueError::Thrown(captures[1].clone()));
            }
            heap.string("dynamic".into())
        },
    };
    static GET: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "controlled Date.toJSON current ISO Get after number conversion/throw",
        name: "get toISOString",
        length: 0,
        form: CompiledFunctionForm::Builtin,
        capture_count: 5,
        body: |heap, captures, receiver, _, runtime| {
            assert!(receiver.same_identity(&captures[4]));
            heap.array_push(&captures[0], &[heap.string("get:iso".into())?], runtime)?;
            if matches!(heap.snapshot(&captures[3])?, ValueSnapshot::Boolean(true)) {
                return Err(ValueError::Thrown(captures[2].clone()));
            }
            Ok(captures[1].clone())
        },
    };
    for where_ in ["none", "short", "convert", "get", "call"] {
        let object = heap.object()?;
        let effects = heap.array(vec![])?;
        let marker = heap.object()?;
        let primitive = heap.number(if where_ == "short" { f64::INFINITY } else { 7.0 })?;
        heap.define_own_data(
            &object,
            heap.to_primitive_key(),
            heap.fresh_compiled_function(
                &CONVERT,
                &[effects.clone(), primitive, marker.clone(), heap.boolean(where_ == "convert")?],
            )?,
            runtime,
        )?;
        let callable = heap.fresh_compiled_function(
            &CALL,
            &[effects.clone(), marker.clone(), heap.boolean(where_ == "call")?, object.clone()],
        )?;
        let getter = heap.fresh_compiled_function(
            &GET,
            &[effects.clone(), callable, marker.clone(), heap.boolean(where_ == "get")?, object.clone()],
        )?;
        let ValueSnapshot::Function(getter) = heap.snapshot(&getter)? else {
            panic!("compiled getter missing");
        };
        heap.define_own(
            &object,
            "toISOString".into(),
            PropertyDescriptor::Accessor { get: Some(getter), set: None, enumerable: false, configurable: true },
            runtime,
        )?;
        let result = heap.call(&to_json, &object, &[], runtime);
        if where_ == "none" {
            exact_text(realm, &result?, oracle["dynamic"].as_str().unwrap())?;
        } else if where_ == "short" {
            assert!(matches!(heap.snapshot(&result?)?, ValueSnapshot::Null));
        } else {
            assert!(oracle["identity"][where_].as_bool().unwrap());
            assert!(thrown(result)?.same_identity(&marker));
        }
        if matches!(where_, "none" | "short") {
            let actual = heap.stringify(&effects, StringifyMode::Plain, None, runtime)?.unwrap().to_utf8().unwrap();
            let actual: serde_json::Value = serde_json::from_str(&actual).unwrap();
            assert_eq!(actual, oracle[if where_ == "none" { "effects" } else { "shortEffects" }]);
        }
        receipts.extend([object, effects, marker]);
    }
    Ok(receipts)
}

pub(super) fn observe(default_realm: &Arc<NativeRealm>) -> Result<Vec<ValueHandle>> {
    let date = global(default_realm, "Date")?;
    assert!(date.same_identity(&default_realm.globals.date));
    // Real default Host clock, separate from the controlled timezone profiles.
    let heap = default_realm.values.heap();
    let before = super::milliseconds();
    let actual_date = heap.construct_compiled_function(&date, &[], default_realm.values.runtime())?;
    let getter = method(default_realm, &actual_date, "getTime")?;
    let actual = heap.call(&getter, &actual_date, &[], default_realm.values.runtime())?;
    let after = super::milliseconds();
    assert!((before..=after).contains(&number(default_realm, &actual)?));
    assert!(heap.instance_of(&actual_date, &date, default_realm.values.runtime())?);
    assert!(matches!(
        heap.call(&date, &actual_date, &[], default_realm.values.runtime()),
        Err(ValueError::UnsupportedOperation(_))
    ));
    let mut receipts = vec![actual_date, actual];
    // Necessary regression boundary for the shared call/construct seam.
    static ORDINARY: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "controlled unchanged Ordinary call/primitive construct return",
        name: "ordinary",
        length: 0,
        form: CompiledFunctionForm::Ordinary,
        capture_count: 0,
        body: |heap, _, _, _, _| heap.number(7.0),
    };
    static ARROW: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "controlled unchanged Arrow lexical receiver and no construction",
        name: "arrow",
        length: 0,
        form: CompiledFunctionForm::Arrow,
        capture_count: 0,
        body: |heap, _, receiver, _, _| {
            assert!(matches!(heap.snapshot(receiver)?, ValueSnapshot::Undefined));
            heap.number(8.0)
        },
    };
    static BUILTIN: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "controlled unchanged Builtin no construction",
        name: "builtin",
        length: 0,
        form: CompiledFunctionForm::Builtin,
        capture_count: 0,
        body: |heap, _, _, _, _| heap.number(9.0),
    };
    for (spec, expected) in [(&ORDINARY, 7.0), (&ARROW, 8.0), (&BUILTIN, 9.0)] {
        let function = heap.fresh_compiled_function(spec, &[])?;
        assert_eq!(
            number(default_realm, &heap.call(&function, &heap.object()?, &[], default_realm.values.runtime())?)?,
            expected
        );
        let constructed = heap.construct_compiled_function(&function, &[], default_realm.values.runtime());
        if spec.form == CompiledFunctionForm::Ordinary {
            let instance = constructed?;
            assert!(heap.instance_of(&instance, &function, default_realm.values.runtime())?);
        } else {
            assert!(matches!(constructed, Err(ValueError::TypeError(_))));
        }
    }
    static FOREIGN_CONSTRUCTOR: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "controlled NativeConstructor foreign-return admission",
        name: "foreignConstructor",
        length: 0,
        form: CompiledFunctionForm::NativeConstructor(|_, _, _, _, _| ValueHeap::new()?.object()),
        capture_count: 0,
        body: |heap, _, _, _, _| heap.undefined(),
    };
    let foreign = heap.fresh_compiled_function(&FOREIGN_CONSTRUCTOR, &[])?;
    assert!(matches!(
        heap.construct_compiled_function(&foreign, &[], default_realm.values.runtime()),
        Err(ValueError::ForeignHeap)
    ));
    static THROW: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "controlled Date original ToNumber/copy-skip marker",
        name: "toPrimitive",
        length: 1,
        form: CompiledFunctionForm::Builtin,
        capture_count: 1,
        body: |_, captures, _, _, _| Err(ValueError::Thrown(captures[0].clone())),
    };
    static REENTRANT: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "controlled Date setDate ToNumber reentry and original t snapshot",
        name: "toPrimitive",
        length: 1,
        form: CompiledFunctionForm::Builtin,
        capture_count: 3,
        body: |heap, captures, _, arguments, runtime| {
            assert!(matches!(heap.snapshot(&arguments[0])?, ValueSnapshot::String(s) if s.equals_ascii("number")));
            heap.array_push(&captures[2], &[arguments[0].clone()], runtime)?;
            heap.call(&captures[1], &captures[0], &[heap.number(50.0)?], runtime)?;
            heap.number(2.0)
        },
    };
    for (offset, source) in
        [(0, include_str!("utc-date-oracle.json")), (28_800, include_str!("shanghai-date-oracle.json"))]
    {
        let oracle: serde_json::Value = serde_json::from_str(source).unwrap();
        assert_eq!(oracle["bun_version"], "1.4.0");
        assert_eq!(oracle["bun_revision"], "34cbb9a40b4bd1bd767d134a7065e66c2432a676");
        let calls = Arc::new(AtomicUsize::new(0));
        let host = Arc::new(FixedHost { offset: FixedOffset::east_opt(offset).unwrap(), now_calls: calls.clone() });
        let weak_host = Arc::downgrade(&host);
        let realm = NativeRealm::prepare_with_date_host(host.clone())?;
        drop(host);
        let heap = realm.values.heap().clone();
        let runtime = realm.values.runtime();
        let date = global(&realm, "Date")?;
        let prototype = method(&realm, &date, "prototype")?;
        receipts.extend(json_family(&realm, &date, &prototype)?);
        let get_time = method(&realm, &prototype, "getTime")?;
        assert!(matches!(heap.own_descriptor(&date, &"prototype".into(), runtime)?, Some(PropertyDescriptor::Data {
            value, writable: false, enumerable: false, configurable: false,
        }) if value.same_identity(&prototype)));
        assert!(method(&realm, &prototype, "constructor")?.same_identity(&date));
        assert_eq!(number(&realm, &method(&realm, &date, "length")?)?, 7.0);
        let names = oracle["methods"].as_array().unwrap();
        for row in oracle["rows"].as_array().unwrap() {
            let instance =
                heap.construct_compiled_function(&date, &[heap.number(oracle_number(&row["input"]))?], runtime)?;
            fields(&realm, &instance, names, row["fields"].as_array().unwrap())?;
            assert!(heap.own_keys(&instance, runtime)?.is_empty());
            receipts.push(instance);
        }
        let now = method(&realm, &date, "now")?;
        assert_eq!(number(&realm, &heap.call(&now, &heap.null()?, &[], runtime)?)?, 1_704_067_200_123.0);
        let current = heap.construct_compiled_function(&date, &[], runtime)?;
        assert_eq!(calls.load(Ordering::Relaxed), 2);
        let marker = heap.object()?;
        let throwing = heap.fresh_compiled_function(&THROW, &[marker.clone()])?;
        heap.define_own_data(&current, heap.to_primitive_key(), throwing.clone(), runtime)?;
        heap.define_own_data(&current, "valueOf".into(), throwing.clone(), runtime)?;
        let copy = heap.construct_compiled_function(&date, &[current.clone()], runtime)?;
        assert!(!copy.same_identity(&current));
        let set_date = method(&realm, &prototype, "setDate")?;
        let day = heap.call(&method(&realm, &copy, "getDate")?, &copy, &[], runtime)?;
        let cutoff = heap.call(&set_date, &copy, &[heap.number(number(&realm, &day)? - 4.0)?], runtime)?;
        expected_number(&realm, &cutoff, &oracle["cutoff_returned"])?;
        fields(&realm, &copy, names, oracle["cutoff_fields"].as_array().unwrap())?;
        expected_number(&realm, &heap.call(&get_time, &current, &[], runtime)?, &oracle["current_time"])?;
        for row in oracle["day_cases"].as_array().unwrap() {
            let instance = heap.construct_compiled_function(&date, &[heap.number(1_704_067_200_123.0)?], runtime)?;
            let result = heap.call(&set_date, &instance, &[heap.number(oracle_number(&row["day"]))?], runtime)?;
            expected_number(&realm, &result, &row["returned"])?;
            fields(&realm, &instance, names, row["fields"].as_array().unwrap())?;
            receipts.extend([instance, result]);
        }
        let target = heap.construct_compiled_function(&date, &[heap.number(1_704_067_200_123.0)?], runtime)?;
        let effects = heap.array(vec![])?;
        let argument = heap.object()?;
        heap.define_own_data(
            &argument,
            heap.to_primitive_key(),
            heap.fresh_compiled_function(&REENTRANT, &[target.clone(), set_date.clone(), effects.clone()])?,
            runtime,
        )?;
        let reentrant = heap.call(&set_date, &target, &[argument], runtime)?;
        expected_number(&realm, &reentrant, &oracle["reentrant"]["returned"])?;
        fields(&realm, &target, names, oracle["reentrant"]["fields"].as_array().unwrap())?;
        assert_eq!(heap.array_length(&effects, runtime)?, 1);
        assert!(
            matches!(heap.snapshot(&heap.get(&effects, &"0".into(), runtime)?)?, ValueSnapshot::String(s) if s.equals_ascii("number"))
        );
        let invalid = heap.construct_compiled_function(&date, &[heap.number(f64::NAN)?], runtime)?;
        let argument = heap.object()?;
        heap.define_own_data(&argument, heap.to_primitive_key(), throwing, runtime)?;
        assert!(thrown(heap.call(&set_date, &invalid, &[argument.clone()], runtime))?.same_identity(&marker));
        let fake = heap.object_with_prototype(Some(&prototype))?;
        heap.define_own_data(&fake, "DateValue".into(), heap.number(0.0)?, runtime)?;
        for receiver in [heap.object()?, fake, heap.null()?, prototype.clone()] {
            let error = thrown(heap.call(&get_time, &receiver, &[], runtime))?;
            assert!(heap.instance_of(&error, &realm.globals.errors.type_error, runtime)?);
            assert!(
                matches!(heap.snapshot(&method(&realm, &error, "message")?)?, ValueSnapshot::String(s) if s.equals_ascii("Type error"))
            );
            // Receiver validation precedes the throwing argument conversion.
            let early = thrown(heap.call(&set_date, &receiver, &[argument.clone()], runtime))?;
            assert!(!early.same_identity(&marker));
            assert!(heap.instance_of(&early, &realm.globals.errors.type_error, runtime)?);
            receipts.extend([error, early]);
        }
        receipts.extend([current.clone(), copy, cutoff, target, reentrant, invalid]);
        let weak_values = Arc::downgrade(&realm.values);
        let retained_runtime = realm.runtime.clone();
        drop(realm);
        assert!(weak_host.upgrade().is_none(), "Date runtime/captures cannot retain the Host owner");
        assert!(weak_values.upgrade().is_none(), "Date domain cannot retain SessionValues");
        assert!(matches!(
            heap.call(&get_time, &current, &[], retained_runtime.as_ref()),
            Err(ValueError::ExpiredCapability(_))
        ));
    }
    eprintln!(
        "Date logger coarse family: real default clock; UTC/+08 Bun fields/copy/setDate/ISO/JSON/original throws/owner expiry"
    );
    Ok(receipts)
}
