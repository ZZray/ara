//! Fixed OMP 596f2da ast-edit.ts input program (MIT; THIRD_PARTY_NOTICES.md).
//! Original argument objects, map methods, entry getters and iterators stay in
//! the original Session realm. Only native AST/OS string leaves are projected.

use crate::native_realm::ast::NativeAstModule;
use ara_agent::{ActualProviderError, SessionValues};
use ara_rpc::{
    WireString,
    value_heap::{
        CompiledFunctionForm, CompiledFunctionSpec, ValueError, ValueHandle, ValueHeap, ValueKind, ValueRuntime,
        ValueSnapshot,
    },
};
use std::sync::Arc;

fn tool_error(
    heap: &Arc<ValueHeap>,
    constructor: &ValueHandle,
    message: WireString,
    runtime: &dyn ValueRuntime,
) -> ValueError {
    match heap.string(message).and_then(|message| heap.construct(constructor, &[message], runtime)) {
        Ok(value) => ValueError::Thrown(value),
        Err(error) => error,
    }
}
static MAP_OP: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da ast-edit.ts (entry,index)=>{if(entry.pat.length===0)throw ToolError;return[entry.pat,entry.out]}",
    name: "",
    length: 2,
    form: CompiledFunctionForm::Arrow,
    capture_count: 1,
    body: |heap, captures, _, arguments, runtime| {
        let entry = arguments.first().cloned().unwrap_or(heap.undefined()?);
        let pat = heap.get(&entry, &"pat".into(), runtime)?;
        let length = heap.get(&pat, &"length".into(), runtime)?;
        if matches!(heap.snapshot(&length)?,ValueSnapshot::Number(value) if value==0.0) {
            let index = arguments.get(1).cloned().unwrap_or(heap.undefined()?);
            let index = heap.to_string(&index, runtime)?;
            let mut message = WireString::from("`ops[").units().to_vec();
            message.extend_from_slice(index.units());
            message.extend_from_slice(WireString::from("].pat` must be a non-empty pattern").units());
            return Err(tool_error(heap, &captures[0], WireString::from_units(message), runtime));
        }
        let pat = heap.get(&entry, &"pat".into(), runtime)?;
        let out = heap.get(&entry, &"out".into(), runtime)?;
        heap.array(vec![pat, out])
    },
};
static NORMALIZE_PATH: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da path-utils.ts normalizePathLikeInput and stripOuterDoubleQuotes",
    name: "normalizePathLikeInput",
    length: 1,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 0,
    body: |heap, _, _, arguments, runtime| {
        let input = arguments.first().cloned().unwrap_or(heap.undefined()?);
        let trim = heap.get(&input, &"trim".into(), runtime)?;
        let input = heap.call(&trim, &input, &[], runtime)?;
        let starts = heap.get(&input, &"startsWith".into(), runtime)?;
        if heap.truthy(&heap.call(&starts, &input, &[heap.string("\"".into())?], runtime)?)? {
            let ends = heap.get(&input, &"endsWith".into(), runtime)?;
            if heap.truthy(&heap.call(&ends, &input, &[heap.string("\"".into())?], runtime)?)? {
                let length = heap.get(&input, &"length".into(), runtime)?;
                if heap.to_number(&length, runtime)? > 1.0 {
                    let slice = heap.get(&input, &"slice".into(), runtime)?;
                    return heap.call(&slice, &input, &[heap.number(1.0)?, heap.number(-1.0)?], runtime);
                }
            }
        }
        Ok(input)
    },
};
pub(super) fn scalar(heap: &Arc<ValueHeap>, value: &ValueHandle) -> Result<String, ActualProviderError> {
    match heap.snapshot(value)? {
        ValueSnapshot::String(value) => {
            value.to_utf8().map_err(|_| ActualProviderError::Unsupported("AST OS UTF16 lone-surrogate boundary"))
        }
        _ => Err(ValueError::TypeError("AST native string leaf must be a string".into()).into()),
    }
}
fn map(
    heap: &Arc<ValueHeap>,
    value: &ValueHandle,
    spec: &'static CompiledFunctionSpec,
    captures: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    let method = heap.get(value, &"map".into(), runtime)?;
    let callback = heap.fresh_compiled_function(spec, captures)?;
    heap.call(&method, value, &[callback], runtime)
}

pub(super) struct AstArguments {
    pub rewrites: ValueHandle,
    pub normalized: ValueHandle,
    pub max_files: usize,
}
pub(super) fn normalize(
    heap: &Arc<ValueHeap>,
    input: &ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    let function = heap.fresh_compiled_function(&NORMALIZE_PATH, &[])?;
    heap.call(&function, &heap.undefined()?, std::slice::from_ref(input), runtime)
}
static EMPTY_PATH: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da path-utils.ts rawPath=>rawPath.length===0",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 0,
    body: |heap, _, _, arguments, runtime| {
        let input = arguments.first().cloned().unwrap_or(heap.undefined()?);
        let length = heap.get(&input, &"length".into(), runtime)?;
        heap.boolean(matches!(heap.snapshot(&length)?,ValueSnapshot::Number(value) if value==0.0))
    },
};
pub(super) fn check_nonempty(
    heap: &Arc<ValueHeap>,
    paths: &ValueHandle,
    module: &NativeAstModule,
    runtime: &dyn ValueRuntime,
) -> Result<(), ActualProviderError> {
    let some = heap.get(paths, &"some".into(), runtime)?;
    let predicate = heap.fresh_compiled_function(&EMPTY_PATH, &[])?;
    if heap.truthy(&heap.call(&some, paths, &[predicate], runtime)?)? {
        return Err(module.throw("Search scope entries must be non-empty paths or globs".into(), runtime).into());
    }
    Ok(())
}
pub(super) fn prepare_arguments(
    values: &SessionValues,
    args: &ValueHandle,
    module: &NativeAstModule,
    runtime: &dyn ValueRuntime,
) -> Result<AstArguments, ActualProviderError> {
    let heap = values.heap();
    let input = heap.get(args, &"ops".into(), runtime)?;
    let mapped = map(heap, &input, &MAP_OP, std::slice::from_ref(&module.tool_error), runtime)?;
    let length = heap.get(&mapped, &"length".into(), runtime)?;
    if matches!(heap.snapshot(&length)?,ValueSnapshot::Number(value) if value==0.0) {
        return Err(module.throw("`ops` must include at least one op entry".into(), runtime).into());
    }
    let constructor = heap.get(&module.environment, &"Set".into(), runtime)?;
    let seen = heap.construct(&constructor, &[], runtime)?;
    let mut iterator = heap.get_iterator(&mapped, runtime)?;
    while let Some(entry) = heap.iterator_next(&mut iterator, runtime)? {
        let body = (|| {
            let mut pair = heap.get_iterator(&entry, runtime)?;
            let pat = match heap.iterator_next(&mut pair, runtime) {
                Ok(value) => value.unwrap_or(heap.undefined()?),
                Err(error) => {
                    let _ = heap.iterator_close(&mut pair, runtime);
                    return Err(error.into());
                }
            };
            heap.iterator_close(&mut pair, runtime)?;
            let has = heap.get(&seen, &"has".into(), runtime)?;
            if heap.truthy(&heap.call(&has, &seen, std::slice::from_ref(&pat), runtime)?)? {
                let pattern = heap.to_string(&pat, runtime)?;
                let mut message = WireString::from("Duplicate rewrite pattern: ").units().to_vec();
                message.extend_from_slice(pattern.units());
                return Err(module.throw(WireString::from_units(message), runtime).into());
            }
            let add = heap.get(&seen, &"add".into(), runtime)?;
            heap.call(&add, &seen, &[pat], runtime)?;
            Ok::<_, ActualProviderError>(())
        })();
        if let Err(error) = body {
            let _ = heap.iterator_close(&mut iterator, runtime);
            return Err(error);
        }
    }
    // Original global and member Gets are observable, including replacement
    // Object/fromEntries bindings. Keep its actual returned object identity.
    let object = heap.get(&module.environment, &"Object".into(), runtime)?;
    let from_entries = heap.get(&object, &"fromEntries".into(), runtime)?;
    let rewrites = heap.call(&from_entries, &object, &[mapped], runtime)?;
    let max_files = ara_tools::ast_edit::max_files_from_env(std::env::var("ARA_MAX_AST_FILES").ok().as_deref());
    let paths = heap.get(args, &"paths".into(), runtime)?;
    let normalized = map(heap, &paths, &NORMALIZE_PATH, &[], runtime)?;
    check_nonempty(heap, &normalized, module, runtime)?;
    Ok(AstArguments { rewrites, normalized, max_files })
}

/// Fixed resolve.ts toolCallWritePath: conditional repeated Gets remain in the
/// original domain; a plain write is not soft-requirement compliance.
pub fn satisfies(values: &SessionValues, call: &ValueHandle) -> Result<bool, ActualProviderError> {
    let heap = values.heap();
    let runtime = values.runtime();
    let name = heap.get(call, &"name".into(), runtime)?;
    if !heap.object_is(&name, &heap.string(WireString::from("write"))?)? {
        return Ok(false);
    }
    let args = heap.get(call, &"arguments".into(), runtime)?;
    if matches!(heap.snapshot(&args)?, ValueSnapshot::Null | ValueSnapshot::Undefined) {
        return Ok(false);
    }
    let first = heap.get(&args, &"path".into(), runtime)?;
    let path = if matches!(heap.snapshot(&first)?, ValueSnapshot::String(_)) {
        Some(heap.get(&args, &"path".into(), runtime)?)
    } else {
        let alternate = heap.get(&args, &"file_path".into(), runtime)?;
        if matches!(heap.snapshot(&alternate)?, ValueSnapshot::String(_)) {
            Some(heap.get(&args, &"file_path".into(), runtime)?)
        } else {
            None
        }
    };
    Ok(path
        .map(|path| scalar(heap, &path))
        .transpose()?
        .is_some_and(|path| ara_tools::resolve::resolution_device(&path).is_some()))
}

static IS_INTERNAL: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da ast-edit.ts approval paths.every(isInternalUrlPath)",
    name: "isInternalUrlPath",
    length: 1,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 0,
    body: |heap, _, _, arguments, _| {
        let path = arguments.first().cloned().unwrap_or(heap.undefined()?);
        let ValueSnapshot::String(path) = heap.snapshot(&path)? else {
            return Err(ValueError::TypeError("Internal URL path must be a string".into()));
        };
        let path =
            path.to_utf8().map_err(|_| ValueError::UnsupportedOperation("approval internal path lone surrogate"))?;
        let path = if let Some(rest) = path.strip_prefix("local:/").filter(|rest| !rest.starts_with('/')) {
            format!("local://{rest}")
        } else {
            path
        };
        let path =
            path.strip_prefix('@').filter(|rest| rest.starts_with("local:") || rest.contains("://")).unwrap_or(&path);
        heap.boolean(
            [
                "agent://",
                "artifact://",
                "skill://",
                "rule://",
                "security://",
                "local://",
                "mcp://",
                "ssh://",
                "vault://",
            ]
            .iter()
            .any(|prefix| path.starts_with(prefix)),
        )
    },
};
static APPROVAL: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da ast-edit.ts approval",
    name: "approval",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 0,
    body: |heap, _, _, arguments, runtime| {
        let args = arguments.first().cloned().unwrap_or(heap.undefined()?);
        let first = heap.get(&args, &"paths".into(), runtime)?;
        let paths = if heap.kind(&first)? == ValueKind::Array {
            heap.get(&args, &"paths".into(), runtime)?
        } else {
            heap.array(Vec::new())?
        };
        let length = heap.get(&paths, &"length".into(), runtime)?;
        let read = if heap.to_number(&length, runtime)? > 0.0 {
            let every = heap.get(&paths, &"every".into(), runtime)?;
            let predicate = heap.fresh_compiled_function(&IS_INTERNAL, &[])?;
            heap.truthy(&heap.call(&every, &paths, &[predicate], runtime)?)?
        } else {
            false
        };
        heap.string(if read { "read" } else { "write" }.into())
    },
};
fn approval_line(
    heap: &Arc<ValueHeap>,
    prefix: &str,
    value: &ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    let length = heap.get(value, &"length".into(), runtime)?;
    let length = heap.to_number(&length, runtime)?;
    let text = if length <= 2000.0 {
        value.clone()
    } else {
        let slice = heap.get(value, &"slice".into(), runtime)?;
        heap.call(&slice, value, &[heap.number(0.0)?, heap.number(2000.0)?], runtime)?
    };
    let mut units = WireString::from(prefix).units().to_vec();
    units.extend_from_slice(heap.to_string(&text, runtime)?.units());
    let mut text = WireString::from_units(units);
    if length > 2000.0 {
        text.append_str(&format!("[…{}ch elided…]", length - 2000.0));
    }
    heap.string(text)
}
static APPROVAL_DETAILS: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da ast-edit.ts formatApprovalDetails",
    name: "formatApprovalDetails",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 0,
    body: |heap, _, _, arguments, runtime| {
        let args = arguments.first().cloned().unwrap_or(heap.undefined()?);
        let mut lines = Vec::new();
        let first = heap.get(&args, &"ops".into(), runtime)?;
        let ops = if heap.kind(&first)? == ValueKind::Array {
            heap.get(&args, &"ops".into(), runtime)?
        } else {
            heap.array(Vec::new())?
        };
        let first = heap.get(&ops, &"0".into(), runtime)?;
        if heap.truthy(&first)? {
            let pat = heap.get(&first, &"pat".into(), runtime)?;
            lines.push(approval_line(heap, "Pattern: ", &pat, runtime)?);
            let out = heap.get(&first, &"out".into(), runtime)?;
            lines.push(approval_line(heap, "Replacement: ", &out, runtime)?);
            let length = heap.get(&ops, &"length".into(), runtime)?;
            let length = heap.to_number(&length, runtime)?;
            if length > 1.0 {
                let count = heap.to_number(&heap.get(&ops, &"length".into(), runtime)?, runtime)? - 1.0;
                let plural = heap.object_is(&heap.get(&ops, &"length".into(), runtime)?, &heap.number(2.0)?)?;
                lines.push(
                    heap.string(
                        format!("+{} more op{}", ara_prompt::js::f64_to_string(count), if plural { "" } else { "s" })
                            .into(),
                    )?,
                );
            }
        }
        let first = heap.get(&args, &"paths".into(), runtime)?;
        if heap.kind(&first)? == ValueKind::Array {
            let paths = heap.get(&args, &"paths".into(), runtime)?;
            let length = heap.get(&paths, &"length".into(), runtime)?;
            if heap.to_number(&length, runtime)? > 0.0 {
                let paths = heap.get(&args, &"paths".into(), runtime)?;
                let join = heap.get(&paths, &"join".into(), runtime)?;
                let text = heap.call(&join, &paths, &[heap.string(", ".into())?], runtime)?;
                lines.push(approval_line(heap, "Paths: ", &text, runtime)?);
            }
        }
        heap.array(lines)
    },
};
pub fn install(heap: &Arc<ValueHeap>, source: &ValueHandle, runtime: &dyn ValueRuntime) -> Result<(), ValueError> {
    for (key, value) in [
        ("label", heap.string("AST Edit".into())?),
        ("summary", heap.string("Perform AST-aware code edits (structural refactoring)".into())?),
        ("strict", heap.boolean(true)?),
        ("deferrable", heap.boolean(true)?),
        ("loadMode", heap.string("discoverable".into())?),
        ("approval", heap.fresh_compiled_function(&APPROVAL, &[])?),
        ("formatApprovalDetails", heap.fresh_compiled_function(&APPROVAL_DETAILS, &[])?),
    ] {
        heap.define_own_data(source, key.into(), value, runtime)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ara_rpc::value_heap::{PropertyDescriptor, PropertyKey};
    static GET: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "AST actual getter receipt fixture",
        name: "get",
        length: 0,
        form: CompiledFunctionForm::Ordinary,
        capture_count: 3,
        body: |heap, captures, _, _, runtime| {
            let trace = heap.get(&captures[0], &"trace".into(), runtime)?;
            let mut trace = heap.to_string(&trace, runtime)?;
            let marker = heap.to_string(&captures[2], runtime)?;
            let mut units = trace.units().to_vec();
            units.extend_from_slice(marker.units());
            trace = WireString::from_units(units);
            heap.define_own_data(&captures[0], "trace".into(), heap.string(trace)?, runtime)?;
            Ok(captures[1].clone())
        },
    };
    fn getter(
        values: &SessionValues,
        target: &ValueHandle,
        key: &str,
        state: &ValueHandle,
        value: ValueHandle,
        marker: &str,
    ) {
        let heap = values.heap();
        let function =
            heap.fresh_compiled_function(&GET, &[state.clone(), value, heap.string(marker.into()).unwrap()]).unwrap();
        let ValueSnapshot::Function(function) = heap.snapshot(&function).unwrap() else { panic!() };
        heap.define_own(
            target,
            key.into(),
            PropertyDescriptor::Accessor { get: Some(function), set: None, enumerable: true, configurable: true },
            values.runtime(),
        )
        .unwrap();
    }
    fn mark(
        heap: &Arc<ValueHeap>,
        state: &ValueHandle,
        marker: &str,
        runtime: &dyn ValueRuntime,
    ) -> Result<(), ValueError> {
        let text = heap.get(state, &"trace".into(), runtime)?;
        let mut units = heap.to_string(&text, runtime)?.units().to_vec();
        units.extend(WireString::from(marker).units());
        heap.define_own_data(state, "trace".into(), heap.string(WireString::from_units(units))?, runtime)
    }
    static KEY: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "grouped AST original ToPropertyKey observation",
        name: "",
        length: 1,
        form: CompiledFunctionForm::Builtin,
        capture_count: 2,
        body: |heap, captures, _, args, runtime| {
            mark(heap, &captures[0], "K", runtime)?;
            heap.define_own_data(&captures[0], "hint".into(), args[0].clone(), runtime)?;
            Ok(captures[1].clone())
        },
    };
    static THROW_KEY: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "grouped AST original key throw identity",
        name: "",
        length: 1,
        form: CompiledFunctionForm::Builtin,
        capture_count: 2,
        body: |heap, captures, _, _, runtime| {
            mark(heap, &captures[0], "K", runtime)?;
            Err(ValueError::Thrown(captures[1].clone()))
        },
    };
    static NEXT: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "grouped AST original iterator next",
        name: "next",
        length: 0,
        form: CompiledFunctionForm::Builtin,
        capture_count: 2,
        body: |heap, captures, _, _, runtime| {
            mark(heap, &captures[0], "N", runtime)?;
            let result = heap.object()?;
            let first = heap.get(&captures[0], &"first".into(), runtime)?;
            let done = heap.truthy(&first)?;
            heap.define_own_data(&captures[0], "first".into(), heap.boolean(true)?, runtime)?;
            heap.define_own_data(&result, "done".into(), heap.boolean(done)?, runtime)?;
            heap.define_own_data(&result, "value".into(), captures[1].clone(), runtime)?;
            Ok(result)
        },
    };
    static CLOSE: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "grouped AST original iterator close",
        name: "return",
        length: 0,
        form: CompiledFunctionForm::Builtin,
        capture_count: 1,
        body: |heap, captures, _, _, runtime| {
            mark(heap, &captures[0], "R", runtime)?;
            heap.object()
        },
    };
    static RETURN: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "grouped AST original supplied binding",
        name: "",
        length: 0,
        form: CompiledFunctionForm::Ordinary,
        capture_count: 1,
        body: |_, captures, _, _, _| Ok(captures[0].clone()),
    };
    static SUPER_ARGUMENTS: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "grouped ToolError exact super argument observation",
        name: "super",
        length: 1,
        form: CompiledFunctionForm::Ordinary,
        capture_count: 2,
        body: |heap, captures, _, args, runtime| {
            heap.define_own_data(&captures[1], "argc".into(), heap.number(args.len() as f64)?, runtime)?;
            heap.define_own_data(
                &captures[1],
                "messageArg".into(),
                args.first().cloned().unwrap_or(heap.undefined()?),
                runtime,
            )?;
            Ok(captures[0].clone())
        },
    };
    static COLLECTION_ADDER_GET: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "grouped native collection branded receiver before adder Get",
        name: "get adder",
        length: 0,
        form: CompiledFunctionForm::Builtin,
        capture_count: 2,
        body: |heap, captures, receiver, _, runtime| {
            mark(heap, &captures[0], "A", runtime)?;
            // The inherited intrinsic size getter requires initialized slots.
            let size = heap.get(receiver, &"size".into(), runtime)?;
            heap.define_own_data(&captures[0], "initialSize".into(), size, runtime)?;
            heap.define_own_data(&captures[0], "receiver".into(), receiver.clone(), runtime)?;
            heap.define_own_data(receiver, "fromAdder".into(), heap.boolean(true)?, runtime)?;
            Ok(captures[1].clone())
        },
    };
    static COLLECTION_ADD: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "grouped native collection original adder receiver identity",
        name: "adder",
        length: 2,
        form: CompiledFunctionForm::Builtin,
        capture_count: 2,
        body: |heap, captures, receiver, args, runtime| {
            mark(heap, &captures[0], "C", runtime)?;
            let original = heap.get(&captures[0], &"receiver".into(), runtime)?;
            assert!(receiver.same_identity(&original));
            heap.call(&captures[1], receiver, args, runtime)
        },
    };
    static COLLECTION_ITERATOR_GET: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "grouped native collection adder precedes iterator acquisition",
        name: "get iterator",
        length: 0,
        form: CompiledFunctionForm::Builtin,
        capture_count: 2,
        body: |heap, captures, _, _, runtime| {
            mark(heap, &captures[0], "I", runtime)?;
            Ok(captures[1].clone())
        },
    };
    #[test]
    fn ast_edit_actual_getters_metadata_and_resolution_predicate_group() {
        let realm = crate::native_realm::NativeRealm::prepare().unwrap();
        let values = &realm.values;
        let heap = values.heap();
        let runtime = values.runtime();
        let state = heap.object().unwrap();
        heap.define_own_data(&state, "trace".into(), heap.string("".into()).unwrap(), runtime).unwrap();
        let entry = heap.object().unwrap();
        getter(values, &entry, "pat", &state, heap.string("foo($A)".into()).unwrap(), "P");
        getter(values, &entry, "out", &state, heap.string("bar($A)".into()).unwrap(), "T");
        let args = heap.object().unwrap();
        getter(values, &args, "ops", &state, heap.array(vec![entry]).unwrap(), "O");
        getter(values, &args, "paths", &state, heap.array(vec![heap.string("calls.ts".into()).unwrap()]).unwrap(), "S");
        let input = prepare_arguments(values, &args, &realm.ast, runtime).unwrap();
        assert_eq!(
            super::super::ast_edit_native::native_engine_input(heap, &input.rewrites, &realm.ast, runtime).unwrap(),
            vec![("foo($A)".into(), "bar($A)".into())]
        );
        let path = heap.get(&input.normalized, &"0".into(), runtime).unwrap();
        assert_eq!(scalar(heap, &path).unwrap(), "calls.ts");
        let trace = heap.get(&state, &"trace".into(), runtime).unwrap();
        assert_eq!(scalar(heap, &trace).unwrap(), "OPPTS");
        let error = heap.construct(&realm.ast.tool_error, &[heap.string("original".into()).unwrap()], runtime).unwrap();
        assert!(heap.instance_of(&error, &realm.ast.tool_error, runtime).unwrap());
        assert!(heap.instance_of(&error, &realm.globals.errors.error, runtime).unwrap());
        let render = heap.get(&error, &"render".into(), runtime).unwrap();
        assert_eq!(scalar(heap, &heap.call(&render, &error, &[], runtime).unwrap()).unwrap(), "original");
        assert!(matches!(
            heap.call(&realm.ast.tool_error, &heap.undefined().unwrap(), &[], runtime),
            Err(ValueError::Thrown(_))
        ));
        // Original fromEntries bindings: entry Gets precede key coercion;
        // symbol/UTF16/__proto__/collision keys stay in the original heap.
        let object = heap.get(&realm.ast.environment, &"Object".into(), runtime).unwrap();
        let from = heap.get(&object, &"fromEntries".into(), runtime).unwrap();
        heap.define_own_data(&state, "trace".into(), heap.string("".into()).unwrap(), runtime).unwrap();
        let key = heap.object().unwrap();
        let symbol = heap.symbol(Some("key".into())).unwrap();
        heap.define_own_data(
            &key,
            heap.to_primitive_key(),
            heap.fresh_compiled_function(&KEY, &[state.clone(), symbol.clone()]).unwrap(),
            runtime,
        )
        .unwrap();
        let pair = heap.object().unwrap();
        getter(values, &pair, "0", &state, key, "0");
        getter(values, &pair, "1", &state, heap.string("symbol value".into()).unwrap(), "1");
        let utf16 = WireString::from_units(vec![0xd800]);
        let pairs = heap
            .array(vec![
                pair,
                heap.array(vec![heap.string(utf16.clone()).unwrap(), heap.string("units".into()).unwrap()]).unwrap(),
                heap.array(vec![heap.string("__proto__".into()).unwrap(), heap.string("own".into()).unwrap()]).unwrap(),
                heap.array(vec![heap.number(1.0).unwrap(), heap.string("first".into()).unwrap()]).unwrap(),
                heap.array(vec![heap.string("1".into()).unwrap(), heap.string("last".into()).unwrap()]).unwrap(),
            ])
            .unwrap();
        let result = heap.call(&from, &object, &[pairs], runtime).unwrap();
        assert_eq!(scalar(heap, &heap.get(&state, &"trace".into(), runtime).unwrap()).unwrap(), "01K");
        assert_eq!(scalar(heap, &heap.get(&state, &"hint".into(), runtime).unwrap()).unwrap(), "string");
        assert_eq!(
            scalar(heap, &heap.get(&result, &PropertyKey::Symbol(heap.symbol_key(&symbol).unwrap()), runtime).unwrap())
                .unwrap(),
            "symbol value"
        );
        assert!(heap.own_descriptor(&result, &PropertyKey::String(utf16), runtime).unwrap().unwrap().enumerable());
        assert_eq!(scalar(heap, &heap.get(&result, &"__proto__".into(), runtime).unwrap()).unwrap(), "own");
        assert!(heap.prototype(&result, runtime).unwrap().unwrap().same_identity(&heap.object_prototype()));
        assert_eq!(scalar(heap, &heap.get(&result, &"1".into(), runtime).unwrap()).unwrap(), "last");
        // Abrupt key coercion closes the same outer iterator and preserves the
        // original thrown handle rather than reconstructing its name/message.
        heap.define_own_data(&state, "trace".into(), heap.string("".into()).unwrap(), runtime).unwrap();
        heap.define_own_data(&state, "first".into(), heap.boolean(false).unwrap(), runtime).unwrap();
        let reason = heap.object().unwrap();
        let key = heap.object().unwrap();
        heap.define_own_data(
            &key,
            heap.to_primitive_key(),
            heap.fresh_compiled_function(&THROW_KEY, &[state.clone(), reason.clone()]).unwrap(),
            runtime,
        )
        .unwrap();
        let pair = heap.object().unwrap();
        getter(values, &pair, "0", &state, key, "0");
        getter(values, &pair, "1", &state, heap.string("out".into()).unwrap(), "1");
        let iterator = heap.object().unwrap();
        heap.define_own_data(
            &iterator,
            "next".into(),
            heap.fresh_compiled_function(&NEXT, &[state.clone(), pair]).unwrap(),
            runtime,
        )
        .unwrap();
        heap.define_own_data(
            &iterator,
            "return".into(),
            heap.fresh_compiled_function(&CLOSE, &[state.clone()]).unwrap(),
            runtime,
        )
        .unwrap();
        let iterable = heap.object().unwrap();
        heap.define_own_data(
            &iterable,
            heap.iterator_key(),
            heap.fresh_compiled_function(&RETURN, &[iterator]).unwrap(),
            runtime,
        )
        .unwrap();
        let Err(ValueError::Thrown(thrown)) = heap.call(&from, &object, &[iterable], runtime) else {
            panic!("original key failure absent")
        };
        assert!(thrown.same_identity(&reason));
        assert_eq!(scalar(heap, &heap.get(&state, &"trace".into(), runtime).unwrap()).unwrap(), "N01KR");
        // Dynamic superclass and explicit distinct newTarget remain observable.
        let replacement = heap.object().unwrap();
        let superclass = heap.fresh_compiled_function(&SUPER_ARGUMENTS, &[replacement.clone(), state.clone()]).unwrap();
        heap.set_prototype(&realm.ast.tool_error, Some(&superclass), runtime).unwrap();
        let derived = heap.construct(&realm.ast.tool_error, &[heap.string("unused".into()).unwrap()], runtime).unwrap();
        assert!(derived.same_identity(&replacement));
        assert_eq!(scalar(heap, &heap.get(&derived, &"name".into(), runtime).unwrap()).unwrap(), "ToolError");
        let no_message = heap.construct(&realm.ast.tool_error, &[], runtime).unwrap();
        assert!(no_message.same_identity(&replacement));
        assert_eq!(heap.to_number(&heap.get(&state, &"argc".into(), runtime).unwrap(), runtime).unwrap(), 1.0);
        assert_eq!(heap.kind(&heap.get(&state, &"messageArg".into(), runtime).unwrap()).unwrap(), ValueKind::Undefined);
        heap.set_prototype(&realm.ast.tool_error, Some(&realm.globals.errors.error), runtime).unwrap();
        let new_target = heap.fresh_compiled_function(&RETURN, &[replacement]).unwrap();
        let prototype = heap.get(&new_target, &"prototype".into(), runtime).unwrap();
        let derived = heap
            .construct_with_new_target(
                &realm.ast.tool_error,
                &[heap.string("same receiver".into()).unwrap()],
                &new_target,
                runtime,
            )
            .unwrap();
        assert!(heap.prototype(&derived, runtime).unwrap().unwrap().same_identity(&prototype));
        assert_eq!(scalar(heap, &heap.get(&derived, &"message".into(), runtime).unwrap()).unwrap(), "same receiver");
        // Install Map through the same original logger primitive owner.
        let storage = realm.context().prepare_storage(heap).unwrap();
        let _logger =
            crate::native_realm::logger::NativeLoggerBindings::prepare_sync(heap, &realm.globals, &storage, runtime)
                .unwrap();
        // Native Set/Map initialize the original allocated derived receiver
        // before adder Get and iterable acquisition, including slots/identity.
        for (name, adder_key, trace) in [("Set", "add", "AIC"), ("Map", "set", "AI01C")] {
            heap.define_own_data(&state, "trace".into(), heap.string("".into()).unwrap(), runtime).unwrap();
            let constructor = heap.get(&realm.ast.environment, &name.into(), runtime).unwrap();
            let intrinsic_proto = heap.get(&constructor, &"prototype".into(), runtime).unwrap();
            let derived_proto = heap.object_with_prototype(Some(&intrinsic_proto)).unwrap();
            let original_add = heap.get(&intrinsic_proto, &adder_key.into(), runtime).unwrap();
            let adder = heap.fresh_compiled_function(&COLLECTION_ADD, &[state.clone(), original_add.clone()]).unwrap();
            let accessor = heap.fresh_compiled_function(&COLLECTION_ADDER_GET, &[state.clone(), adder]).unwrap();
            let ValueSnapshot::Function(accessor_target) = heap.snapshot(&accessor).unwrap() else {
                panic!("adder getter")
            };
            heap.define_own(
                &derived_proto,
                adder_key.into(),
                PropertyDescriptor::Accessor {
                    get: Some(accessor_target),
                    set: None,
                    enumerable: false,
                    configurable: true,
                },
                runtime,
            )
            .unwrap();
            let new_target = heap.fresh_compiled_function(&RETURN, &[heap.object().unwrap()]).unwrap();
            heap.assign(&new_target, "prototype".into(), derived_proto.clone(), runtime).unwrap();
            let element = if name == "Set" {
                heap.string("key".into()).unwrap()
            } else {
                let pair = heap.object().unwrap();
                getter(values, &pair, "0", &state, heap.string("key".into()).unwrap(), "0");
                getter(values, &pair, "1", &state, heap.string("value".into()).unwrap(), "1");
                pair
            };
            let iterable = heap.array(vec![element]).unwrap();
            let iterator = heap.get(&iterable, &heap.iterator_key(), runtime).unwrap();
            let accessor = heap.fresh_compiled_function(&COLLECTION_ITERATOR_GET, &[state.clone(), iterator]).unwrap();
            let ValueSnapshot::Function(accessor_target) = heap.snapshot(&accessor).unwrap() else {
                panic!("iterator getter")
            };
            heap.define_own(
                &iterable,
                heap.iterator_key(),
                PropertyDescriptor::Accessor {
                    get: Some(accessor_target),
                    set: None,
                    enumerable: false,
                    configurable: true,
                },
                runtime,
            )
            .unwrap();
            let derived = heap.construct_with_new_target(&constructor, &[iterable], &new_target, runtime).unwrap();
            assert!(heap.prototype(&derived, runtime).unwrap().unwrap().same_identity(&derived_proto));
            assert!(heap.instance_of(&derived, &new_target, runtime).unwrap());
            assert!(heap.instance_of(&derived, &constructor, runtime).unwrap());
            assert!(derived.same_identity(&heap.get(&state, &"receiver".into(), runtime).unwrap()));
            assert_eq!(
                heap.to_number(&heap.get(&state, &"initialSize".into(), runtime).unwrap(), runtime).unwrap(),
                0.0
            );
            assert_eq!(heap.to_number(&heap.get(&derived, &"size".into(), runtime).unwrap(), runtime).unwrap(), 1.0);
            assert!(heap.truthy(&heap.get(&derived, &"fromAdder".into(), runtime).unwrap()).unwrap());
            assert_eq!(scalar(heap, &heap.get(&state, &"trace".into(), runtime).unwrap()).unwrap(), trace);
            // ToolError's dynamically patched Native superclass uses the same
            // allocated ToolError receiver even with an undefined iterable.
            heap.set_prototype(&realm.ast.tool_error, Some(&constructor), runtime).unwrap();
            let derived = heap.construct(&realm.ast.tool_error, &[], runtime).unwrap();
            assert!(heap.instance_of(&derived, &realm.ast.tool_error, runtime).unwrap());
            let tool_proto = heap.get(&realm.ast.tool_error, &"prototype".into(), runtime).unwrap();
            assert!(heap.prototype(&derived, runtime).unwrap().unwrap().same_identity(&tool_proto));
            let args = if name == "Set" {
                vec![heap.string("branded".into()).unwrap()]
            } else {
                vec![heap.string("branded".into()).unwrap(), heap.number(42.0).unwrap()]
            };
            assert!(heap.call(&original_add, &derived, &args, runtime).unwrap().same_identity(&derived));
            let member = heap.get(&intrinsic_proto, &"has".into(), runtime).unwrap();
            assert!(heap.truthy(&heap.call(&member, &derived, &[args[0].clone()], runtime).unwrap()).unwrap());
            assert_eq!(scalar(heap, &heap.get(&derived, &"name".into(), runtime).unwrap()).unwrap(), "ToolError");
            heap.set_prototype(&realm.ast.tool_error, Some(&realm.globals.errors.error), runtime).unwrap();
        }
        // Original ABI includes inherited enumerable names with nonenumerable
        // own shadows, skips undefined, rereads UTF8 replacement aliases and
        // accepts primitive String maps. Existing acceptance entrance only.
        let proto = heap.object().unwrap();
        heap.define_own_data(&proto, "pattern".into(), heap.string("inherited".into()).unwrap(), runtime).unwrap();
        let rewrites = heap.object_with_prototype(Some(&proto)).unwrap();
        heap.define_own(
            &rewrites,
            "pattern".into(),
            PropertyDescriptor::Data {
                value: heap.string("shadow".into()).unwrap(),
                writable: true,
                enumerable: false,
                configurable: true,
            },
            runtime,
        )
        .unwrap();
        heap.define_own_data(&rewrites, "skip".into(), heap.undefined().unwrap(), runtime).unwrap();
        heap.define_own_data(
            &rewrites,
            PropertyKey::String(WireString::from_units(vec![0xd800])),
            heap.string("wrong".into()).unwrap(),
            runtime,
        )
        .unwrap();
        heap.define_own(
            &rewrites,
            "�".into(),
            PropertyDescriptor::Data {
                value: heap.string("alias".into()).unwrap(),
                writable: true,
                enumerable: false,
                configurable: true,
            },
            runtime,
        )
        .unwrap();
        let projected =
            super::super::ast_edit_native::native_engine_input(heap, &rewrites, &realm.ast, runtime).unwrap();
        assert_eq!(projected, vec![("pattern".into(), "shadow".into()), ("�".into(), "alias".into())]);
        assert_eq!(
            super::super::ast_edit_native::native_engine_input(
                heap,
                &heap.string("abc".into()).unwrap(),
                &realm.ast,
                runtime
            )
            .unwrap(),
            vec![("0".into(), "a".into()), ("1".into(), "b".into()), ("2".into(), "c".into())]
        );
        let invalid = heap.object().unwrap();
        heap.define_own_data(&invalid, "pat".into(), heap.number(42.0).unwrap(), runtime).unwrap();
        let Err(ActualProviderError::Value(ValueError::Thrown(reason))) =
            super::super::ast_edit_native::native_engine_input(heap, &invalid, &realm.ast, runtime)
        else {
            panic!("native strict String failure absent")
        };
        assert!(heap.instance_of(&reason, &realm.globals.errors.error, runtime).unwrap());
        assert_eq!(
            scalar(heap, &heap.get(&reason, &"message".into(), runtime).unwrap()).unwrap(),
            "Failed to convert JavaScript value `Number 42 ` into rust type `String` on AstReplaceOptions.rewrites"
        );
        let source = heap.object().unwrap();
        install(heap, &source, runtime).unwrap();
        assert_eq!(scalar(heap, &heap.get(&source, &"label".into(), runtime).unwrap()).unwrap(), "AST Edit");
        let approval = heap.get(&source, &"approval".into(), runtime).unwrap();
        for (path, kind) in [("skill://demo/a.ts", "read"), ("local://a.ts", "read"), ("a.ts", "write")] {
            let args = heap.import_serde(&serde_json::json!({"paths":[path]})).unwrap();
            assert_eq!(scalar(heap, &heap.call(&approval, &source, &[args], runtime).unwrap()).unwrap(), kind);
        }
        for (path, want) in
            [("xd://resolve", true), ("XD://reject", true), ("ordinary.ts", false), ("xd://Resolve", false)]
        {
            let call = heap.import_serde(&serde_json::json!({"name":"write","arguments":{"path":path}})).unwrap();
            assert_eq!(satisfies(values, &call).unwrap(), want);
        }
    }
}
