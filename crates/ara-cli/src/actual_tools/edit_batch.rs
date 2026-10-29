//! Fixed OMP 596f2da tools/render-utils.ts getLspBatchRequest (MIT).
//! Original context, array methods, names and getters stay in their own realm.
//! The fresh native ApplyRequest contains only the selected batch scalars.

use ara_edit::ApplyRequest;
use ara_rpc::value_heap::{
    CompiledFunctionForm, CompiledFunctionSpec, ValueError, ValueHandle, ValueHeap, ValueRuntime, ValueSnapshot,
};
use std::sync::Arc;

fn strict_equal(heap: &Arc<ValueHeap>, left: &ValueHandle, right: &ValueHandle) -> Result<bool, ValueError> {
    Ok(match (heap.snapshot(left)?, heap.snapshot(right)?) {
        (ValueSnapshot::Undefined, ValueSnapshot::Undefined) | (ValueSnapshot::Null, ValueSnapshot::Null) => true,
        (ValueSnapshot::Boolean(left), ValueSnapshot::Boolean(right)) => left == right,
        (ValueSnapshot::Number(left), ValueSnapshot::Number(right)) => left == right,
        (ValueSnapshot::String(left), ValueSnapshot::String(right)) => left == right,
        (ValueSnapshot::BigInt(left), ValueSnapshot::BigInt(right)) => left == right,
        (ValueSnapshot::Symbol { key: left, .. }, ValueSnapshot::Symbol { key: right, .. }) => left == right,
        (
            ValueSnapshot::Opaque { semantic_identity: left, .. },
            ValueSnapshot::Opaque { semantic_identity: right, .. },
        ) => left == right,
        _ => left.same_identity(right),
    })
}

fn is_write(heap: &Arc<ValueHeap>, call: &ValueHandle, runtime: &dyn ValueRuntime) -> Result<bool, ValueError> {
    let name = heap.get(call, &"name".into(), runtime)?;
    Ok(matches!(heap.snapshot(&name)?, ValueSnapshot::String(name)
        if name == ara_rpc::WireString::from("edit") || name == ara_rpc::WireString::from("write")))
}

static OTHER_WRITE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da render-utils.ts (call,index)=>index!==toolCall.index&&LSP_BATCH_TOOLS.has(call.name)",
    name: "",
    length: 2,
    form: CompiledFunctionForm::Arrow,
    capture_count: 1,
    body: |heap, captures, _, arguments, runtime| {
        let call = arguments.first().cloned().unwrap_or(heap.undefined()?);
        let index = arguments.get(1).cloned().unwrap_or(heap.undefined()?);
        let current = heap.get(&captures[0], &"index".into(), runtime)?;
        heap.boolean(!strict_equal(heap, &index, &current)? && is_write(heap, &call, runtime)?)
    },
};

static LATER_WRITE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da render-utils.ts call=>LSP_BATCH_TOOLS.has(call.name)",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 0,
    body: |heap, _, _, arguments, runtime| {
        let call = arguments.first().cloned().unwrap_or(heap.undefined()?);
        heap.boolean(is_write(heap, &call, runtime)?)
    },
};

static GET_BATCH: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da tools/render-utils.ts getLspBatchRequest",
    name: "getLspBatchRequest",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 0,
    body: |heap, _, _, arguments, runtime| {
        let call = arguments.first().cloned().unwrap_or(heap.undefined()?);
        if !heap.truthy(&call)? {
            return heap.undefined();
        }
        let calls = heap.get(&call, &"toolCalls".into(), runtime)?;
        let some = heap.get(&calls, &"some".into(), runtime)?;
        let other = heap.fresh_compiled_function(&OTHER_WRITE, &[call.clone()])?;
        let has_other = heap.call(&some, &calls, &[other], runtime)?;
        if !heap.truthy(&has_other)? {
            return heap.undefined();
        }
        let calls = heap.get(&call, &"toolCalls".into(), runtime)?;
        let slice = heap.get(&calls, &"slice".into(), runtime)?;
        let index = heap.get(&call, &"index".into(), runtime)?;
        let start = heap.add(&index, &heap.number(1.0)?, runtime)?;
        let tail = heap.call(&slice, &calls, &[start], runtime)?;
        let some = heap.get(&tail, &"some".into(), runtime)?;
        let later = heap.fresh_compiled_function(&LATER_WRITE, &[])?;
        let has_later = heap.call(&some, &tail, &[later], runtime)?;
        let result = heap.object()?;
        let id = heap.get(&call, &"batchId".into(), runtime)?;
        heap.define_own_data(&result, "id".into(), id, runtime)?;
        heap.define_own_data(&result, "flush".into(), heap.boolean(!heap.truthy(&has_later)?)?, runtime)?;
        Ok(result)
    },
};

/// Call after pool lookup / non-streaming stringify, at the original position.
/// Caller must have already admitted this exact ToolContext lineage.
pub(super) fn prepare(
    heap: &Arc<ValueHeap>,
    context: &ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<ApplyRequest, ValueError> {
    let call = if matches!(heap.snapshot(context)?, ValueSnapshot::Undefined | ValueSnapshot::Null) {
        heap.undefined()?
    } else {
        heap.get(context, &"toolCall".into(), runtime)?
    };
    let function = heap.fresh_compiled_function(&GET_BATCH, &[])?;
    let result = heap.call(&function, &heap.undefined()?, &[call], runtime)?;
    if matches!(heap.snapshot(&result)?, ValueSnapshot::Undefined) {
        return Ok(ApplyRequest::default());
    }
    let id = heap.get(&result, &"id".into(), runtime)?;
    let lsp_batch_id = match heap.snapshot(&id)? {
        ValueSnapshot::Undefined | ValueSnapshot::Null => None,
        ValueSnapshot::String(id) => {
            Some(id.to_utf8().map_err(|_| ValueError::TypeError("LSP batch ID contains unpaired UTF16".into()))?)
        }
        _ => return Err(ValueError::TypeError("LSP batch ID must be a string".into())),
    };
    Ok(ApplyRequest { lsp_batch_id, lsp_flush: heap.truthy(&heap.get(&result, &"flush".into(), runtime)?)? })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ara_rpc::value_heap::{NoRuntime, PropertyDescriptor};
    use serde_json::json;

    static THROW_CAPTURE: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "Test original batchId Getter throws retained graph value",
        name: "get batchId",
        length: 0,
        form: CompiledFunctionForm::Builtin,
        capture_count: 1,
        body: |_, captures, _, _, _| Err(ValueError::Thrown(captures[0].clone())),
    };

    #[test]
    fn fixed_original_graph_batch_metadata_family() {
        let heap = ValueHeap::new().unwrap();
        let runtime = NoRuntime;
        for (names, index, expected) in [
            (vec!["edit"], 0, None),
            (vec!["edit", "read"], 0, None),
            (vec!["edit", "write"], 0, Some(false)),
            (vec!["edit", "write"], 1, Some(true)),
            (vec!["write", "read", "edit"], 2, Some(true)),
        ] {
            let calls: Vec<_> = names.iter().map(|name| json!({"name":name})).collect();
            let context =
                heap.import_serde(&json!({"toolCall":{"batchId":"batch","index":index,"toolCalls":calls}})).unwrap();
            let request = prepare(&heap, &context, &runtime).unwrap();
            assert_eq!(request.lsp_batch_id.as_deref(), expected.map(|_| "batch"));
            assert_eq!(request.lsp_flush, expected.unwrap_or(false));
        }
        assert!(prepare(&heap, &heap.undefined().unwrap(), &runtime).unwrap().lsp_batch_id.is_none());
        let malformed = heap.import_serde(&json!({"toolCall":{"index":0,"toolCalls":{}}})).unwrap();
        assert!(prepare(&heap, &malformed, &runtime).is_err());

        let thrown = heap.object().unwrap();
        for (calls, should_throw) in
            [(json!([{"name":"edit"}]), false), (json!([{"name":"edit"},{"name":"write"}]), true)]
        {
            let context = heap.import_serde(&json!({"toolCall":{"index":0,"toolCalls":calls}})).unwrap();
            let call = heap.get(&context, &"toolCall".into(), &runtime).unwrap();
            let getter = heap.fresh_compiled_function(&THROW_CAPTURE, &[thrown.clone()]).unwrap();
            let ValueSnapshot::Function(target) = heap.snapshot(&getter).unwrap() else {
                panic!();
            };
            heap.define_own(
                &call,
                "batchId".into(),
                PropertyDescriptor::Accessor { get: Some(target), set: None, enumerable: true, configurable: true },
                &runtime,
            )
            .unwrap();
            let result = prepare(&heap, &context, &runtime);
            if should_throw {
                let Err(ValueError::Thrown(actual)) = result else {
                    panic!("original Getter throw was replaced");
                };
                assert!(actual.same_identity(&thrown));
            } else {
                assert!(result.unwrap().lsp_batch_id.is_none()); // batchId is not read
            }
        }
    }
}
