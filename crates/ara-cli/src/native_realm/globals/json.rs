//! Original Native JSON.stringify binding. Argument/graph semantics are owned
//! by the unique ValueHeap encoder; captures are same-realm error prototypes.
use super::*;

pub(super) fn install(
    heap: &Arc<ValueHeap>,
    json: &ValueHandle,
    prototypes: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<()> {
    nonenumerable(heap, json, "stringify", function(heap, &STRINGIFY, prototypes)?, runtime)
}
static STRINGIFY: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "ECMA Native JSON.stringify original arguments and graph operations consumed by OMP596f2da",
    name: "stringify",
    length: 3,
    form: CompiledFunctionForm::Builtin,
    capture_count: 4,
    body: |heap, prototypes, _, arguments, runtime| {
        native(heap, prototypes, runtime, || {
            let value = argument(heap, arguments, 0)?;
            match heap.stringify_native(&value, arguments.get(1), arguments.get(2), runtime)? {
                Some(output) => heap.string(output),
                None => heap.undefined(),
            }
        })
    },
};
