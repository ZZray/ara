//! Original Graph notice consumer of fixed OMP596f2da tool-call-loop-redirect
//! and stream-guards. MIT; notices in THIRD_PARTY_NOTICES.md.
use ara_ai::graph_values::GraphValueFactory;
use ara_prompt::actual::NativePromptModule;
use ara_rpc::value_heap::{ValueError, ValueHandle};

const TEMPLATE: &str = include_str!("tool_call_loop_redirect.md");
type Result<T> = std::result::Result<T, ValueError>;

/// Only the source-selected fixed template can create this custom notice.
/// Its content/details/timestamp stay original same-realm Graph values.
/// This function must be called after detector state commit, outside locks.
pub fn tool_call_loop(
    factory: &GraphValueFactory,
    module: &NativePromptModule,
    detection: &ValueHandle,
) -> Result<ValueHandle> {
    let heap = factory.heap();
    let runtime = factory.runtime();
    heap.adopt(detection)?;
    heap.adopt(module.prompt())?;
    heap.adopt(module.date())?;
    // renderToolCallLoopRedirect: callee Get precedes context field Gets.
    let render = heap.get(module.prompt(), &"render".into(), runtime)?;
    let template = heap.string(TEMPLATE.into())?;
    let context = heap.object()?;
    for (output, input) in [("tool_name", "toolName"), ("count", "count"), ("arguments_summary", "argumentsSummary")] {
        let value = heap.get(detection, &input.into(), runtime)?;
        heap.define_own_data(&context, output.into(), value, runtime)?;
    }
    let result = heap.get(detection, &"resultSummary".into(), runtime)?;
    let result = if heap.truthy(&result)? { result } else { heap.string("(no text result)".into())? };
    heap.define_own_data(&context, "result_summary".into(), result, runtime)?;
    let content = heap.call(&render, module.prompt(), &[template, context], runtime)?;
    // Details are read again after render, which may mutate detection.
    let details = heap.object()?;
    for key in ["toolName", "count", "argumentsSummary", "resultSummary"] {
        let value = heap.get(detection, &key.into(), runtime)?;
        heap.define_own_data(&details, key.into(), value, runtime)?;
    }
    let custom = heap.object()?;
    for (key, value) in [
        ("role", heap.string("custom".into())?),
        ("customType", heap.string(super::TOOL_CALL_LOOP_REDIRECT_TYPE.into())?),
        ("content", content),
        ("display", heap.boolean(false)?),
        ("details", details),
        ("attribution", heap.string("agent".into())?),
    ] {
        heap.define_own_data(&custom, key.into(), value, runtime)?;
    }
    let now = heap.get(module.date(), &"now".into(), runtime)?;
    let timestamp = heap.call(&now, module.date(), &[], runtime)?;
    heap.define_own_data(&custom, "timestamp".into(), timestamp, runtime)?;
    Ok(custom)
}
