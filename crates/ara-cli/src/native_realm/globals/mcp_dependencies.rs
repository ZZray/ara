//! Neutral dependencies of fixed OMP MCP tool-bridge.ts/errors.ts.
//! Root explicitly installs them on the existing Logger-prepared globals;
//! Object/Bun/Set and input graph identities are never replaced.
use super::*;
use ara_rpc::value_heap::PropertyKey;

fn missing_member(
    heap: &Arc<ValueHeap>,
    owner: &ValueHandle,
    name: &str,
    spec: &'static CompiledFunctionSpec,
    prototypes: &[ValueHandle],
    enumerable: bool,
    configurable: bool,
    runtime: &dyn ValueRuntime,
) -> Result<()> {
    if heap.own_descriptor(owner, &name.into(), runtime)?.is_some() {
        return Ok(());
    }
    heap.define_own(
        owner,
        name.into(),
        PropertyDescriptor::Data { value: function(heap, spec, prototypes)?, writable: true, enumerable, configurable },
        runtime,
    )
}
pub(super) fn install(heap: &Arc<ValueHeap>, globals: &NativeGlobals, runtime: &dyn ValueRuntime) -> Result<()> {
    let object = heap.get(&globals.environment, &"Object".into(), runtime)?;
    let prototypes = &globals.errors.prototypes;
    missing_member(heap, &object, "keys", &KEYS, prototypes, false, true, runtime)?;
    missing_member(heap, &object, "hasOwn", &HAS_OWN, prototypes, false, true, runtime)?;
    let bun = heap.get(&globals.environment, &"Bun".into(), runtime)?;
    missing_member(heap, &bun, "deepEquals", &DEEP_EQUALS, prototypes, true, false, runtime)?;
    if heap.own_descriptor(&globals.environment, &"WeakSet".into(), runtime)?.is_none() {
        let prototype = heap.weak_set_prototype()?;
        let constructor = heap.fresh_native_constructor_with_prototype(&WEAK_SET, prototypes, &prototype)?;
        nonenumerable(heap, &prototype, "constructor", constructor.clone(), runtime)?;
        nonenumerable(heap, &globals.environment, "WeakSet", constructor, runtime)?;
    }
    Ok(())
}
static KEYS: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "ECMAScript Object.keys; fixed OMP596f2da MCP tool-bridge.ts:86",
    name: "keys",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 4,
    body: |heap, prototypes, _, args, runtime| {
        native(heap, prototypes, runtime, || {
            let value = argument(heap, args, 0)?;
            let keys = heap.own_enumerable_string_keys(&value, runtime)?;
            let mut values = Vec::new();
            for key in keys {
                values.try_reserve(1).map_err(|_| ValueError::Allocation)?;
                values.push(heap.string(key)?);
            }
            heap.array(values)
        })
    },
};
static HAS_OWN: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "ECMAScript Object.hasOwn; fixed OMP596f2da MCP tool-bridge.ts:97,121",
    name: "hasOwn",
    length: 2,
    form: CompiledFunctionForm::Builtin,
    capture_count: 4,
    body: |heap, prototypes, _, args, runtime| {
        native(heap, prototypes, runtime, || {
            let value = argument(heap, args, 0)?;
            // ToObject precedes ToPropertyKey, including nullish failure before
            // any key valueOf/toString callback. Same-heap primitive boxing is
            // provided by own_descriptor's existing string index/length domain.
            if matches!(heap.kind(&value)?, ValueKind::Null | ValueKind::Undefined) {
                return Err(ValueError::TypeError("Cannot convert undefined or null to object".into()));
            }
            let key = heap.to_primitive(&argument(heap, args, 1)?, true, runtime)?;
            let key = if heap.kind(&key)? == ValueKind::Symbol {
                PropertyKey::Symbol(heap.symbol_key(&key)?)
            } else {
                PropertyKey::String(heap.to_string(&key, runtime)?)
            };
            heap.boolean(heap.has_own(&value, &key, runtime)?)
        })
    },
};
fn weak_set_construct(
    heap: &Arc<ValueHeap>,
    prototypes: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    native(heap, prototypes, runtime, || heap.new_weak_set(&argument(heap, args, 0)?, runtime))
}
static WEAK_SET: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "ECMAScript WeakSet; fixed OMP596f2da MCP tool-bridge.ts:130;errors.ts:100,125",
    name: "WeakSet",
    length: 0,
    form: CompiledFunctionForm::NativeConstructor(weak_set_construct),
    capture_count: 4,
    body: |heap, prototypes, _, _, runtime| {
        native(heap, prototypes, runtime, || Err(ValueError::TypeError("Constructor WeakSet requires 'new'".into())))
    },
};
static DEEP_EQUALS: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Selected Bun1.4.0 MCP comparison reference; fixed OMP596f2da tool-bridge.ts:273",
    name: "deepEquals",
    length: 2,
    form: CompiledFunctionForm::Builtin,
    capture_count: 4,
    body: |heap, prototypes, _, args, runtime| {
        native(heap, prototypes, runtime, || {
            let left = argument(heap, args, 0)?;
            let right = argument(heap, args, 1)?;
            // Fixed MCP supplies exactly these two arguments, never strict mode.
            heap.boolean(heap.mcp_deep_equals(&left, &right, runtime)?)
        })
    },
};
