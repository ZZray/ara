//! Shared raw tool-schema projection for provider-context normalization.
//! Fixed OMP `packages/ai/src/utils/schema/wire.ts::toolWireSchema`, at
//! 596f2da7101178214aa27a753529d15e6b7ad91d (MIT; see THIRD_PARTY_NOTICES.md).

use serde_json::Value;
use std::fmt;

/// The existing schema conversion depth bound was exceeded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolSchemaError;

impl fmt::Display for ToolSchemaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("tool schema exceeds the supported conversion depth")
    }
}

impl std::error::Error for ToolSchemaError {}

/// Upgrade a raw JSON schema and apply shared wire postprocessing before
/// intent injection and provider-specific sanitization. The authored schema
/// remains unchanged, including literal instance data.
pub fn tool_wire_schema(parameters: &Value) -> Result<Value, ToolSchemaError> {
    let upgraded = crate::schema_draft::upgrade_json_schema(parameters, 0).map_err(|()| ToolSchemaError)?;
    crate::schema_wire::postprocess_json_wire_schema(&upgraded, 0).map_err(|()| ToolSchemaError)
}

/// Shared schema identity cache; entries never become host properties.
pub fn graph_schema_stamp_in<F>(
    heap: &std::sync::Arc<ara_rpc::value_heap::ValueHeap>,
    runtime: &dyn ara_rpc::value_heap::ValueRuntime,
    target: &ara_rpc::value_heap::ValueHandle,
    key: &ara_rpc::value_heap::PropertyKey,
    compute: F,
) -> Result<ara_rpc::value_heap::ValueHandle, ara_rpc::value_heap::ValueError>
where F: FnOnce(&ara_rpc::value_heap::ValueHandle) -> Result<ara_rpc::value_heap::ValueHandle, ara_rpc::value_heap::ValueError> {
    crate::schema_stamps::stamp_with_execution(crate::graph_values::GraphExecution::new(heap, runtime), target, key, compute)
}
