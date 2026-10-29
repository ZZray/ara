//! Selected dependency observations for the existing MCP coarse module run.
//! Root wires this helper into that existing family; no new #[test] entrance.
use ara_cli::native_realm::NativeGlobals;
use ara_rpc::{
    WireString,
    value_heap::{
        CompiledFunctionForm, CompiledFunctionSpec, NoRuntime, PropertyDescriptor, PropertyKey, ValueError,
        ValueHandle, ValueHeap, ValueRuntime, ValueSnapshot,
    },
};
use std::sync::Arc;
type Result<T> = std::result::Result<T, ValueError>;
static TRACED_GETTER: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Controlled MCP dependency getter reference",
    name: "get value",
    length: 0,
    form: CompiledFunctionForm::Arrow,
    capture_count: 3,
    body: |heap, captures, _, _, runtime| {
        heap.array_push(&captures[0], &[captures[1].clone()], runtime)?;
        Ok(captures[2].clone())
    },
};
fn object(heap: &Arc<ValueHeap>, fields: &[(&str, ValueHandle)], runtime: &dyn ValueRuntime) -> Result<ValueHandle> {
    let value = heap.object()?;
    for (key, field) in fields {
        heap.define_own_data(&value, (*key).into(), field.clone(), runtime)?;
    }
    Ok(value)
}
fn getter(
    heap: &Arc<ValueHeap>,
    value: &ValueHandle,
    key: &str,
    trace: &ValueHandle,
    label: &str,
    result: ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<()> {
    let function =
        heap.fresh_compiled_function(&TRACED_GETTER, &[trace.clone(), heap.string(label.into())?, result])?;
    let ValueSnapshot::Function(target) = heap.snapshot(&function)? else {
        return Err(ValueError::InvalidNode);
    };
    heap.define_own(
        value,
        key.into(),
        PropertyDescriptor::Accessor { get: Some(target), set: None, enumerable: true, configurable: true },
        runtime,
    )
}
fn method(
    heap: &Arc<ValueHeap>,
    receiver: &ValueHandle,
    name: &str,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    heap.call(&heap.get(receiver, &name.into(), runtime)?, receiver, args, runtime)
}
fn equals(
    heap: &Arc<ValueHeap>,
    bun: &ValueHandle,
    left: ValueHandle,
    right: ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<bool> {
    heap.truthy(&method(heap, bun, "deepEquals", &[left, right], runtime)?)
}
fn strings(heap: &Arc<ValueHeap>, value: &ValueHandle, runtime: &dyn ValueRuntime) -> Result<Vec<WireString>> {
    let mut values = Vec::new();
    for index in 0..heap.array_length(value, runtime)? {
        let ValueSnapshot::String(text) =
            heap.snapshot(&heap.get(value, &WireString::from(index.to_string()).into(), runtime)?)?
        else {
            return Err(ValueError::InvalidNode);
        };
        values.push(text);
    }
    Ok(values)
}
pub struct McpDependencyObservation {
    pub object_checks: usize,
    pub weak_set_checks: usize,
    pub comparison_rows: usize,
}
pub fn observe_mcp_dependencies(
    heap: &Arc<ValueHeap>,
    globals: &NativeGlobals,
    runtime: &dyn ValueRuntime,
) -> Result<McpDependencyObservation> {
    let original_bun = heap.get(&globals.environment, &"Bun".into(), runtime)?;
    let original_object = heap.get(&globals.environment, &"Object".into(), runtime)?;
    let original_map = heap.get(&globals.environment, &"Map".into(), runtime)?;
    globals.install_mcp_dependencies(runtime)?;
    assert!(heap.get(&globals.environment, &"Bun".into(), runtime)?.same_identity(&original_bun));
    assert!(heap.get(&globals.environment, &"Object".into(), runtime)?.same_identity(&original_object));
    assert!(heap.get(&globals.environment, &"Map".into(), runtime)?.same_identity(&original_map));
    let trace = heap.array(vec![])?;
    let inherited = object(heap, &[("inherited", heap.number(1.0)?)], runtime)?;
    let value = heap.object_with_prototype(Some(&inherited))?;
    getter(heap, &value, "x", &trace, "getter:x", heap.number(1.0)?, runtime)?;
    let symbol = heap.symbol(Some("own".into()))?;
    heap.define_own_data(&value, PropertyKey::Symbol(heap.symbol_key(&symbol)?), heap.number(2.0)?, runtime)?;
    let keys = method(heap, &original_object, "keys", &[value.clone()], runtime)?;
    assert_eq!(strings(heap, &keys, runtime)?, vec![WireString::from("x")]);
    assert_eq!(heap.array_length(&trace, runtime)?, 0);
    assert!(heap.truthy(&method(
        heap,
        &original_object,
        "hasOwn",
        &[value.clone(), heap.string("x".into())?],
        runtime
    )?)?);
    assert!(!heap.truthy(&method(
        heap,
        &original_object,
        "hasOwn",
        &[value.clone(), heap.string("inherited".into())?],
        runtime
    )?)?);
    assert!(heap.truthy(&method(heap, &original_object, "hasOwn", &[value, symbol], runtime)?)?);
    assert_eq!(heap.array_length(&trace, runtime)?, 0);
    assert_eq!(
        heap.array_length(&method(heap, &original_object, "keys", &[heap.string("ab".into())?], runtime)?, runtime)?,
        2
    );
    let ValueError::Thrown(reason) = method(heap, &original_object, "keys", &[heap.null()?], runtime).unwrap_err()
    else {
        panic!("Object.keys null must retain TypeError");
    };
    assert!(heap.instance_of(&reason, &globals.errors.type_error, runtime)?);

    let constructor = heap.get(&globals.environment, &"WeakSet".into(), runtime)?;
    let weak_set = heap.construct(&constructor, &[], runtime)?;
    let key = heap.object()?;
    let other = heap.object()?;
    assert!(method(heap, &weak_set, "add", &[key.clone()], runtime)?.same_identity(&weak_set));
    method(heap, &weak_set, "add", &[key.clone()], runtime)?;
    assert!(heap.truthy(&method(heap, &weak_set, "has", &[key.clone()], runtime)?)?);
    assert!(!heap.truthy(&method(heap, &weak_set, "has", &[other], runtime)?)?);
    assert!(!heap.truthy(&method(heap, &weak_set, "has", &[heap.number(1.0)?], runtime)?)?);
    assert!(heap.truthy(&method(heap, &weak_set, "delete", &[key.clone()], runtime)?)?);
    assert!(!heap.truthy(&method(heap, &weak_set, "has", &[key], runtime)?)?);
    assert!(matches!(heap.snapshot(&heap.get(&weak_set, &"size".into(), runtime)?)?, ValueSnapshot::Undefined));
    assert!(heap.get_iterator(&weak_set, runtime).is_err());
    // Internal membership adds no strong heap lease: arena dies with leases.
    let temporary = ValueHeap::new()?;
    let lease = Arc::downgrade(&temporary);
    let native = temporary.new_weak_set(&temporary.undefined()?, &NoRuntime)?;
    method(&temporary, &native, "add", &[temporary.object()?], &NoRuntime)?;
    drop(native);
    drop(temporary);
    assert!(lease.upgrade().is_none());

    let bun = &original_bun;
    let missing = heap.object()?;
    let undefined = object(heap, &[("x", heap.undefined()?)], runtime)?;
    assert!(equals(heap, bun, missing.clone(), undefined.clone(), runtime)?);
    assert!(equals(heap, bun, undefined, missing, runtime)?);
    assert!(equals(heap, bun, heap.array(vec![heap.undefined()?])?, heap.array_with_length(1)?, runtime)?);
    assert!(equals(heap, bun, heap.array(vec![])?, heap.array_with_length(1)?, runtime)?);
    assert!(!equals(heap, bun, heap.number(0.0)?, heap.number(-0.0)?, runtime)?);
    assert!(equals(heap, bun, heap.number(f64::NAN)?, heap.number(f64::NAN)?, runtime)?);
    let inherited = object(heap, &[("x", heap.number(1.0)?)], runtime)?;
    let extended = heap.object_with_prototype(Some(&inherited))?;
    assert!(equals(heap, bun, heap.object()?, extended.clone(), runtime)?);
    assert!(!equals(heap, bun, object(heap, &[("x", heap.number(1.0)?)], runtime)?, extended.clone(), runtime)?);
    heap.define_own_data(&extended, "x".into(), heap.number(1.0)?, runtime)?;
    assert!(equals(heap, bun, object(heap, &[("x", heap.number(1.0)?)], runtime)?, extended, runtime)?);
    let alias = object(heap, &[("v", heap.number(1.0)?)], runtime)?;
    let aliased = object(heap, &[("a", alias.clone()), ("b", alias)], runtime)?;
    let distinct = object(
        heap,
        &[
            ("a", object(heap, &[("v", heap.number(1.0)?)], runtime)?),
            ("b", object(heap, &[("v", heap.number(1.0)?)], runtime)?),
        ],
        runtime,
    )?;
    assert!(equals(heap, bun, aliased, distinct, runtime)?);
    let a = heap.object()?;
    let b = heap.object()?;
    let c = heap.object()?;
    let d = heap.object()?;
    heap.define_own_data(&a, "next".into(), a.clone(), runtime)?;
    heap.define_own_data(&b, "next".into(), b.clone(), runtime)?;
    heap.define_own_data(&c, "next".into(), d.clone(), runtime)?;
    heap.define_own_data(&d, "next".into(), c.clone(), runtime)?;
    assert!(equals(heap, bun, a.clone(), b, runtime)?);
    assert!(!equals(heap, bun, a, c, runtime)?);
    for kind in ["different-key", "same-key", "same-key-different-value", "extra-key"] {
        let trace = heap.array(vec![])?;
        let left = heap.object()?;
        let right = heap.object()?;
        getter(heap, &left, "x", &trace, "left:x", heap.number(1.0)?, runtime)?;
        let key = if kind == "different-key" { "y" } else { "x" };
        getter(
            heap,
            &right,
            key,
            &trace,
            &format!("right:{key}"),
            heap.number(if kind == "same-key-different-value" { 2.0 } else { 1.0 })?,
            runtime,
        )?;
        if kind == "extra-key" {
            heap.define_own_data(&right, "extra".into(), heap.number(2.0)?, runtime)?;
        }
        assert_eq!(equals(heap, bun, left, right, runtime)?, kind == "same-key");
        assert_eq!(
            strings(heap, &trace, runtime)?,
            if kind == "different-key" {
                vec![WireString::from("left:x")]
            } else {
                vec!["left:x".into(), "right:x".into()]
            }
        );
    }
    let extra = heap.array(vec![heap.number(1.0)?])?;
    heap.define_own_data(&extra, "x".into(), heap.number(2.0)?, runtime)?;
    assert!(equals(heap, bun, heap.array(vec![heap.number(1.0)?])?, extra, runtime)?);
    Ok(McpDependencyObservation { object_checks: 6, weak_set_checks: 10, comparison_rows: 17 })
}
