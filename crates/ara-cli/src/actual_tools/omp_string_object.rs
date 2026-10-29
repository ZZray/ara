//! Narrow fixed OMP596f2da `type({ key: "string" })` import.
//! Copyright (c) 2025-2026 Can Bölük; Copyright (c) 2026 Stencil Labs, Inc.
//! MIT; see THIRD_PARTY_NOTICES.md.
//! Only the required string property/no morph/extras keep branches used by
//! original WRITE Local4 are imported. This is not full omptype delivery.
use ara_agent::SessionValues;
use ara_rpc::{
    WireString,
    value_heap::{
        CompiledFunctionForm, CompiledFunctionSpec, ValueError, ValueHandle, ValueHeap, ValueKind,
        ValueRuntime, ValueSnapshot,
    },
};
use std::sync::Arc;
type Result<T> = std::result::Result<T, ValueError>;
fn get(
    heap: &Arc<ValueHeap>,
    v: &ValueHandle,
    k: &str,
    r: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    heap.get(v, &k.into(), r)
}
fn own(
    heap: &Arc<ValueHeap>,
    v: &ValueHandle,
    k: &str,
    x: ValueHandle,
    r: &dyn ValueRuntime,
) -> Result<()> {
    heap.define_own_data(v, k.into(), x, r)
}
fn text(heap: &Arc<ValueHeap>, v: &ValueHandle) -> Result<WireString> {
    match heap.snapshot(v)? {
        ValueSnapshot::String(s) => Ok(s),
        _ => Err(ValueError::TypeError("Expected a string".into())),
    }
}
fn actual(heap: &Arc<ValueHeap>, v: &ValueHandle) -> Result<&'static str> {
    Ok(match heap.kind(v)? {
        ValueKind::Null => "null",
        ValueKind::Undefined => "undefined",
        ValueKind::String => "a string",
        ValueKind::Number => "a number",
        ValueKind::Boolean => "boolean",
        ValueKind::BigInt => "a bigint",
        ValueKind::Symbol => "a symbol",
        ValueKind::Function => "a function",
        _ => "an object",
    })
}
fn errors_construct(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    receiver: &ValueHandle,
    args: &[ValueHandle],
    r: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let path = args.first().cloned().unwrap_or(heap.array(Vec::new())?);
    let expected = args
        .get(1)
        .cloned()
        .unwrap_or(heap.string("a string".into())?);
    let data = args.get(2).cloned().unwrap_or(heap.undefined()?);
    if args
        .get(3)
        .is_some_and(|v| !matches!(heap.kind(v), Ok(ValueKind::Undefined)))
    {
        return Err(ValueError::UnsupportedOperation(
            "omptype error formatting config is outside Local4 import",
        ));
    }
    let missing = data.same_identity(&captures[0]);
    let mut prefix = String::new();
    for i in 0..heap.array_length(&path, r)? {
        let key = text(heap, &get(heap, &path, &i.to_string(), r)?)?
            .to_utf8()
            .map_err(|_| ValueError::UnsupportedOperation("omptype error path UTF16 display"))?;
        if !prefix.is_empty() {
            prefix.push('.')
        }
        prefix.push_str(&key)
    }
    let expected_text = text(heap, &expected)?
        .to_utf8()
        .map_err(|_| ValueError::UnsupportedOperation("omptype expectation UTF16 display"))?;
    let problem = if missing {
        format!("must be {expected_text} (was missing)")
    } else {
        format!("must be {expected_text} (was {})", actual(heap, &data)?)
    };
    let message = if prefix.is_empty() {
        problem.clone()
    } else {
        format!("{prefix} {problem}")
    };
    let entry = heap.object()?;
    for (k, v) in [
        ("path", path),
        ("expected", expected),
        ("data", data),
        ("message", heap.string(message.clone().into())?),
        ("problem", heap.string(problem.into())?),
    ] {
        own(heap, &entry, k, v, r)?
    }
    let entries = heap.array(vec![entry.clone()])?;
    own(heap, receiver, "0", entry, r)?;
    own(heap, receiver, "length", heap.number(1.0)?, r)?;
    own(heap, receiver, "summary", heap.string(message.into())?, r)?;
    own(
        heap,
        receiver,
        "map",
        heap.fresh_compiled_function(&ERRORS_MAP, &[entries])?,
        r,
    )?;
    Ok(receiver.clone())
}
static ERRORS: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da omptype/errors.ts OmpErrors single-error constructor subset",
    name: "OmpErrors",
    length: 4,
    form: CompiledFunctionForm::NativeConstructor(errors_construct),
    capture_count: 1,
    body: |_, _, _, _, _| {
        Err(ValueError::TypeError(
            "Class constructor OmpErrors cannot be invoked without new".into(),
        ))
    },
};
static ERRORS_MAP: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da omptype/errors.ts OmpErrors.map array consumer",
    name: "map",
    length: 1,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 1,
    body: |heap, c, this, args, r| {
        let callback = args.first().cloned().unwrap_or(heap.undefined()?);
        let result = heap.array(Vec::new())?;
        for index in 0..heap.array_length(&c[0], r)? {
            let entry = get(heap, &c[0], &index.to_string(), r)?;
            let value = heap.call(
                &callback,
                &heap.undefined()?,
                &[entry, heap.number(index as f64)?, this.clone()],
                r,
            )?;
            own(heap, &result, &index.to_string(), value, r)?;
        }
        Ok(result)
    },
};
static TYPE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da omptype/type.ts type -> parseDef object required string -> makeType empty steps",
    name: "type",
    length: 1,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 3,
    body: |heap, c, _, args, r| {
        if args.len() != 1 {
            return Err(ValueError::UnsupportedOperation(
                "omptype Local4 import requires one definition argument",
            ));
        }
        let definition = args.first().cloned().unwrap_or(heap.undefined()?);
        if heap.kind(&definition)? != ValueKind::Object {
            return Err(ValueError::UnsupportedOperation(
                "omptype import supports only required-string object definitions",
            ));
        }
        let keys = heap.own_enumerable_string_keys(&definition, r)?;
        if keys.len() != 1 {
            return Err(ValueError::UnsupportedOperation(
                "omptype import supports the single-property Local4 definitions",
            ));
        }
        let key = &keys[0];
        let value = heap.get(&definition, &key.clone().into(), r)?;
        if !text(heap, &value)?.equals_ascii("string")
            || key
                .units()
                .iter()
                .any(|v| matches!(*v, 63 | 91 | 93 | 43 | 46))
        {
            return Err(ValueError::UnsupportedOperation(
                "omptype import supports required string keys without DSL syntax",
            ));
        }
        let keys = heap.array(vec![heap.string(key.clone())?])?;
        let schema =
            heap.fresh_compiled_function(&VALIDATE, &[keys.clone(), c[0].clone(), c[2].clone()])?;
        own(heap, &schema, "run", schema.clone(), r)?;
        own(
            heap,
            &schema,
            "toJsonSchema",
            heap.fresh_compiled_function(&EMIT, &[keys])?,
            r,
        )?;
        own(
            heap,
            &schema,
            "assert",
            heap.fresh_compiled_function(&ASSERT, &[c[0].clone(), c[1].clone()])?,
            r,
        )?;
        Ok(schema)
    },
};
static VALIDATE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da omptype/interp.ts visitNode object/string no morph extras keep",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 3,
    body: |heap, c, _, args, r| {
        let data = args.first().cloned().unwrap_or(heap.undefined()?);
        let reject = |path: ValueHandle, expected: &str, value: ValueHandle| {
            heap.construct(&c[1], &[path, heap.string(expected.into())?, value], r)
        };
        if !matches!(heap.kind(&data)?, ValueKind::Object | ValueKind::Array) {
            return reject(heap.array(Vec::new())?, "an object", data);
        }
        let key = text(heap, &get(heap, &c[0], "0", r)?)?;
        let present = heap.has(&data, &key.clone().into(), r)?;
        if !present {
            return reject(
                heap.array(vec![heap.string(key)?])?,
                "a string",
                c[2].clone(),
            );
        }
        let value = heap.get(&data, &key.clone().into(), r)?;
        if heap.kind(&value)? != ValueKind::String {
            return reject(heap.array(vec![heap.string(key)?])?, "a string", value);
        }
        Ok(data)
    },
};
static EMIT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da omptype/json-schema.ts emitObject required order extras keep and irToJsonSchema options",
    name: "toJsonSchema",
    length: 1,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 1,
    body: |heap, c, _, args, r| {
        let properties = heap.object()?;
        let required = heap.array(Vec::new())?;
        let key = text(heap, &get(heap, &c[0], "0", r)?)?;
        let string = heap.object()?;
        own(heap, &string, "type", heap.string("string".into())?, r)?;
        heap.define_own_data(&properties, key.clone().into(), string, r)?;
        own(heap, &required, "0", heap.string(key)?, r)?;
        let schema = heap.object()?;
        for (k, v) in [
            ("type", heap.string("object".into())?),
            ("properties", properties),
            ("required", required),
        ] {
            own(heap, &schema, k, v, r)?
        }
        if let Some(options) = args
            .first()
            .filter(|v| !matches!(heap.kind(v), Ok(ValueKind::Null | ValueKind::Undefined)))
        {
            let target = get(heap, options, "target", r)?;
            let dialect = get(heap, options, "dialect", r)?;
            let dialect = if heap.kind(&dialect)? == ValueKind::Null {
                None
            } else if heap.kind(&dialect)? != ValueKind::Undefined {
                Some(dialect)
            } else if let ValueSnapshot::String(target) = heap.snapshot(&target)? {
                if target.equals_ascii("draft-2020-12") {
                    Some(heap.string("https://json-schema.org/draft/2020-12/schema".into())?)
                } else if target.equals_ascii("draft-07") {
                    Some(heap.string("http://json-schema.org/draft-07/schema#".into())?)
                } else if target
                    .units()
                    .starts_with(&WireString::from("http://").units())
                    || target
                        .units()
                        .starts_with(&WireString::from("https://").units())
                {
                    Some(heap.string(target)?)
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(dialect) = dialect {
                own(heap, &schema, "$schema", dialect, r)?
            }
            let description = get(heap, options, "description", r)?;
            if heap.kind(&description)? != ValueKind::Undefined {
                own(heap, &schema, "description", description, r)?
            }
        }
        Ok(schema)
    },
};
static ASSERT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da omptype/type.ts assert and errors.ts TraversalError consumed branches",
    name: "assert",
    length: 1,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 2,
    body: |heap, c, this, args, r| {
        let run = get(heap, this, "run", r)?;
        let result = heap.call(&run, this, args, r)?;
        if heap.instance_of(&result, &c[0], r)? {
            let message = get(heap, &result, "summary", r)?;
            let error = heap.construct(&c[1], &[message], r)?;
            own(
                heap,
                &error,
                "name",
                heap.string("TraversalError".into())?,
                r,
            )?;
            own(heap, &error, "errors", result, r)?;
            return Err(ValueError::Thrown(error));
        }
        Ok(result)
    },
};
pub(crate) fn prepare(values: &SessionValues, error: &ValueHandle) -> Result<ValueHandle> {
    let heap = values.heap();
    let r = values.runtime();
    let prototype = heap.object()?;
    let missing = heap.symbol(Some("omptype.missing".into()))?;
    let errors =
        heap.fresh_native_constructor_with_prototype(&ERRORS, &[missing.clone()], &prototype)?;
    let imported =
        heap.fresh_compiled_function(&TYPE, &[errors.clone(), error.clone(), missing])?;
    own(heap, &imported, "errors", errors, r)?;
    Ok(imported)
}
