//! Fixed OMP 596f2da Debug/WebSearch declaration and schema suppliers (MIT).
//! Sources: tools/debug.ts, web/search/index.ts and prompts/tools/*.md.
//! Only declaration, validation and approval are ported here. DAP and search
//! provider execution remain OPEN and fail explicitly; no fake result is made.

use ara_agent::{AgentTool, Concurrency, ToolError, ToolOutput, UpdateFn};
use ara_ai::{JsonObject, Tool, ToolExample};
use async_trait::async_trait;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

pub const DEBUG_UNAVAILABLE: &str = "Debug execution backend is not yet available in ARA";
pub const WEB_SEARCH_UNAVAILABLE: &str = "Web search execution backend is not yet available in ARA";
pub struct DebugTool;
pub struct WebSearchTool;

pub fn debug_schema() -> Value {
    json!({"type": "object", "properties": {"action": {"type": "string", "enum": ["launch", "attach", "set_breakpoint", "remove_breakpoint", "set_instruction_breakpoint", "remove_instruction_breakpoint", "data_breakpoint_info", "set_data_breakpoint", "remove_data_breakpoint", "continue", "step_over", "step_in", "step_out", "pause", "evaluate", "stack_trace", "threads", "scopes", "variables", "disassemble", "read_memory", "write_memory", "modules", "loaded_sources", "custom_request", "output", "terminate", "sessions"]}, "program": {"type": "string", "description": "debug target path; Delve accepts Go package directories"}, "args": {"type": "array", "description": "program arguments", "items": {"type": "string"}}, "adapter": {"type": "string", "description": "configured adapter id (gdb, lldb-dap, debugpy, dlv, rdbg, or dap.json entry)"}, "cwd": {"type": "string"}, "file": {"type": "string", "description": "source file"}, "line": {"type": "number", "description": "source line"}, "function": {"type": "string", "description": "function name"}, "name": {"type": "string", "description": "variable or data name"}, "condition": {"type": "string", "description": "breakpoint condition"}, "hit_condition": {"type": "string"}, "expression": {"type": "string", "description": "expression to evaluate"}, "context": {"type": "string", "description": "evaluate context: watch | repl | hover | variables | clipboard"}, "frame_id": {"type": "number"}, "scope_id": {"type": "number", "description": "scope variables reference"}, "variable_ref": {"type": "number", "description": "variable reference"}, "pid": {"type": "number", "description": "process id for attach"}, "port": {"type": "number", "description": "remote attach port"}, "host": {"type": "string", "description": "remote attach host"}, "levels": {"type": "number", "description": "max stack frames"}, "memory_reference": {"type": "string", "description": "memory reference or address"}, "instruction_reference": {"type": "string"}, "instruction_count": {"type": "number"}, "instruction_offset": {"type": "number"}, "count": {"type": "number", "description": "bytes to read"}, "data": {"type": "string", "description": "base64 memory payload"}, "data_id": {"type": "string", "description": "data breakpoint id"}, "access_type": {"type": "string", "enum": ["read", "write", "readWrite"]}, "command": {"type": "string", "description": "custom dap request command"}, "arguments": {"type": "object", "description": "custom request arguments", "additionalProperties": {}}, "offset": {"type": "number"}, "resolve_symbols": {"type": "boolean"}, "allow_partial": {"type": "boolean"}, "start_module": {"type": "number"}, "module_count": {"type": "number"}, "timeout": {"type": "number", "description": "per-request timeout seconds"}}, "required": ["action"]})
}
pub fn web_search_schema() -> Value {
    json!({"type": "object", "properties": {"query": {"type": "string"}, "recency": {"type": "string", "enum": ["day", "week", "month", "year"]}, "limit": {"type": "number"}, "max_tokens": {"type": "number"}, "temperature": {"type": "number"}, "num_search_results": {"type": "number"}}, "required": ["query"]})
}
pub fn debug_examples() -> Vec<ToolExample> {
    serde_json::from_value(json!([{"caption": "Launch and inspect hang", "note": "1. debug(action: \"launch\", program: \"./my_app\")\n2. debug(action: \"set_breakpoint\", file: \"src/main.c\", line: 42)\n3. debug(action: \"continue\")\n4. If the program appears hung: debug(action: \"pause\")\n5. Inspect state with `threads`, `stack_trace`, `scopes`, and `variables`"}, {"caption": "Launch a Python script with debugpy", "call": {"action": "launch", "adapter": "debugpy", "program": "scripts/job.py", "args": ["--flag"]}}, {"caption": "Raw debugger command through repl", "call": {"action": "evaluate", "expression": "info registers", "context": "repl"}}])).expect("fixed authored Debug examples")
}

#[async_trait]
impl AgentTool for DebugTool {
    fn definition(&self) -> Tool {
        Tool { name: "debug".into(), description: include_str!("debug.md").trim_end().into(),
            parameters: debug_schema(), strict: Some(true), examples: Some(debug_examples()), ..Default::default() }
    }
    fn concurrency(&self, _: &JsonObject) -> Concurrency { Concurrency::Exclusive }
    async fn execute(&self, _: &str, _: JsonObject, _: CancellationToken, _: UpdateFn) -> Result<ToolOutput, ToolError> {
        Err(ToolError(DEBUG_UNAVAILABLE.into()))
    }
}
#[async_trait]
impl AgentTool for WebSearchTool {
    fn definition(&self) -> Tool {
        Tool { name: "web_search".into(), description: include_str!("web-search.md").trim_end().into(),
            parameters: web_search_schema(), strict: Some(true), ..Default::default() }
    }
    async fn execute(&self, _: &str, _: JsonObject, _: CancellationToken, _: UpdateFn) -> Result<ToolOutput, ToolError> {
        Err(ToolError(WEB_SEARCH_UNAVAILABLE.into()))
    }
}
