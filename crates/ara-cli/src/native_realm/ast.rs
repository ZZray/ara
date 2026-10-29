//! Fixed OMP596f2da ast-edit.ts imports and tool-errors.ts ToolError module.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; THIRD_PARTY_NOTICES.md.
//! One module per logical Session. Scalars alone enter Host effect workers.
use super::{NativeGlobals, logger::primitives};
use ara_rpc::{
    WireString,
    value_heap::{
        CompiledFunctionForm, CompiledFunctionSpec, DeferredValue, PropertyDescriptor, ValueError, ValueHandle,
        ValueHeap, ValueRuntime,
    },
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
type Result<T> = std::result::Result<T, ValueError>;
pub struct NativeAstModule {
    pub environment: ValueHandle,
    pub tool_error: ValueHandle,
    native_errors: super::NativeErrorClasses,
    staged: Mutex<HashMap<String, DeferredValue>>,
}
/// Only the module can mint this same-heap checked edge. Publication inserts
/// it under the scalar admission fence without Graph capture or restoration.
pub(crate) struct PreparedAstBinding {
    heap_token: u64,
    edge: DeferredValue,
}
impl NativeAstModule {
    pub fn prepare(heap: &Arc<ValueHeap>, globals: &NativeGlobals, runtime: &dyn ValueRuntime) -> Result<Self> {
        primitives::install_ast_collections(heap, &globals.environment, runtime)?;
        let mut prototypes = Vec::new();
        for name in ["Error", "TypeError", "SyntaxError", "RangeError"] {
            prototypes.push(heap.get(
                &heap.get(&globals.environment, &name.into(), runtime)?,
                &"prototype".into(),
                runtime,
            )?);
        }
        let prototype = heap.object_with_prototype(Some(&prototypes[0]))?;
        let constructor = heap.fresh_native_constructor_with_prototype(&TOOL_ERROR, &prototypes, &prototype)?;
        heap.set_prototype(&constructor, Some(&globals.errors.error), runtime)?;
        heap.define_own(
            &prototype,
            "constructor".into(),
            PropertyDescriptor::Data {
                value: constructor.clone(),
                writable: true,
                enumerable: false,
                configurable: true,
            },
            runtime,
        )?;
        heap.define_own(
            &prototype,
            "render".into(),
            PropertyDescriptor::Data {
                value: heap.fresh_compiled_function(&RENDER, &[])?,
                writable: true,
                enumerable: false,
                configurable: true,
            },
            runtime,
        )?;
        Ok(Self {
            environment: globals.environment.clone(),
            tool_error: constructor,
            native_errors: globals.errors.clone(),
            staged: Mutex::new(HashMap::new()),
        })
    }
    pub fn check_heap(&self, heap: &Arc<ValueHeap>) -> Result<()> {
        heap.adopt(&self.environment)?;
        heap.adopt(&self.tool_error)?;
        Ok(())
    }
    pub(crate) fn native_error(&self, message: WireString, runtime: &dyn ValueRuntime) -> Result<ValueError> {
        let heap = self.environment.heap();
        let value = heap.call(&self.native_errors.error, &heap.undefined()?, &[heap.string(message)?], runtime)?;
        Ok(ValueError::Thrown(value))
    }
    pub(crate) fn throw(&self, message: WireString, runtime: &dyn ValueRuntime) -> ValueError {
        let heap = self.environment.heap();
        match heap.string(message).and_then(|message| heap.construct(&self.tool_error, &[message], runtime)) {
            Ok(value) => ValueError::Thrown(value),
            Err(error) => error,
        }
    }
    pub(crate) fn wrap_apply_failure(&self, error: ValueError, runtime: &dyn ValueRuntime) -> ValueError {
        let error = self.native_errors.create_at_native_origin(error, runtime);
        let ValueError::Thrown(reason) = &error else {
            return error;
        };
        let heap = self.environment.heap();
        let result = (|| {
            if heap.instance_of(reason, &self.tool_error, runtime)? {
                return Err(error.clone());
            }
            let constructor = heap.get(&self.environment, &"Error".into(), runtime)?;
            let message = if heap.instance_of(reason, &constructor, runtime)? {
                heap.get(reason, &"message".into(), runtime)?
            } else {
                let string = heap.get(&self.environment, &"String".into(), runtime)?;
                heap.call(&string, &heap.undefined()?, std::slice::from_ref(reason), runtime)?
            };
            let message = heap.to_string(&message, runtime)?;
            let mut units = WireString::from("Apply failed: ").units().to_vec();
            units.extend_from_slice(message.units());
            Err::<ValueHandle, ValueError>(self.throw(WireString::from_units(units), runtime))
        })();
        match result {
            Err(error) => error,
            Ok(_) => unreachable!(),
        }
    }
    pub(crate) fn stage(&self, id: String, rewrites: &ValueHandle) -> Result<()> {
        self.stage_prepared(id, self.prepare_stage(rewrites)?)
    }
    pub(crate) fn prepare_stage(&self, rewrites: &ValueHandle) -> Result<PreparedAstBinding> {
        let heap = self.environment.heap();
        Ok(PreparedAstBinding { heap_token: heap.heap_token(), edge: heap.capture_deferred(rewrites)? })
    }
    pub(crate) fn stage_prepared(&self, id: String, binding: PreparedAstBinding) -> Result<()> {
        if binding.heap_token != self.environment.heap().heap_token() {
            return Err(ValueError::ForeignHeap);
        }
        self.staged.lock().unwrap_or_else(|e| e.into_inner()).insert(id, binding.edge);
        Ok(())
    }
    pub(crate) fn staged(&self, id: &str) -> Result<Option<ValueHandle>> {
        let edge = self.staged.lock().unwrap_or_else(|e| e.into_inner()).get(id).copied();
        edge.map(|edge| self.environment.heap().restore_deferred(&edge)).transpose()
    }
    pub(crate) fn remove(&self, id: &str) {
        self.staged.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
    }
}
static TOOL_ERROR: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da tools/tool-errors.ts class ToolError extends Error constructor(message,context)",
    name: "ToolError",
    length: 2,
    capture_count: 4,
    form: CompiledFunctionForm::DerivedConstructor(|heap, _captures, constructor, new_target, args, runtime| {
        // GetSuperConstructor reads the active class's current [[Prototype]];
        // original Construct reads newTarget.prototype in the superclass at
        // its own position. Patched superclass returns keep their identity.
        let superclass = heap
            .prototype(constructor, runtime)?
            .ok_or_else(|| ValueError::TypeError("Super constructor is null".into()))?;
        let message = args.first().cloned().unwrap_or(heap.undefined()?);
        let receiver = heap.construct_with_new_target(&superclass, &[message], new_target, runtime)?;
        let context = args.get(1).cloned().unwrap_or(heap.undefined()?);
        heap.define_own_data(&receiver, "context".into(), context, runtime)?;
        heap.assign(&receiver, "name".into(), heap.string("ToolError".into())?, runtime)?;
        Ok(receiver)
    }),
    body: |heap, captures, _, _, runtime| {
        let error = ValueError::TypeError("Class constructor ToolError cannot be invoked without 'new'".into());
        let value = heap.object_with_prototype(Some(&captures[1]))?;
        heap.define_own(
            &value,
            "message".into(),
            PropertyDescriptor::Data {
                value: heap.string(match error {
                    ValueError::TypeError(s) => s.into(),
                    _ => unreachable!(),
                })?,
                writable: true,
                enumerable: false,
                configurable: true,
            },
            runtime,
        )?;
        Err(ValueError::Thrown(value))
    },
};
static RENDER: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da tools/tool-errors.ts ToolError.render(){return this.message}",
    name: "render",
    length: 0,
    form: CompiledFunctionForm::Builtin,
    capture_count: 0,
    body: |heap, _, receiver, _, runtime| heap.get(receiver, &"message".into(), runtime),
};
