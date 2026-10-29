//! Fixed OMP 596f2da, packages/ai/src/dialect/catalog.ts (MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See LICENSE.
//!
//! The actual toolWireSchema bridge is a Root-owned SOURCE gate. It is not
//! presumed equal to Tool.parameters and has no fallback in this module.

use super::types::DialectRenderer;
use crate::relaxed_json::FinalValueFactory;
use ara_rpc::WireString;

pub trait ToolCatalogBridge: FinalValueFactory {
    fn tool_wire_schema(&self, tool: &crate::Tool) -> Result<Self::Value, Self::Error>;
    /// Native catalogue uses object literals: define own data, bypassing
    /// inherited setters. This is distinct from relaxed parser assignment.
    fn define_own_data(&self, object: &mut Self::Value, key: WireString, value: Self::Value)
    -> Result<(), Self::Error>;
    /// Plain Native JSON.stringify, WITHOUT utils.stringifyJson's BigInt
    /// replacer. Catalogue and argument rendering have distinct contracts.
    fn stringify_plain(&self, value: &Self::Value) -> Result<Option<String>, Self::Error>;
}

fn string<F: FinalValueFactory>(factory: &F, text: &str) -> Result<F::Value, F::Error> {
    factory.string(WireString::from_units(text.encode_utf16().collect()))
}
fn define<F: ToolCatalogBridge>(
    factory: &F,
    object: &mut F::Value,
    key: &str,
    value: F::Value,
) -> Result<(), F::Error> {
    ToolCatalogBridge::define_own_data(factory, object, WireString::from_units(key.encode_utf16().collect()), value)
}

pub fn render_tool_catalog<F: ToolCatalogBridge>(tools: &[crate::Tool], bridge: &F) -> Result<String, F::Error> {
    let mut lines = Vec::new();
    for tool in tools {
        // Fixed catalog.ts:12-20 object-literal evaluation order. Constructors,
        // property values and define-own writes can all fail at their source step.
        let mut declaration = bridge.object()?;
        define(bridge, &mut declaration, "type", string(bridge, "function")?)?;
        let mut function = bridge.object()?;
        define(bridge, &mut function, "name", string(bridge, &tool.name)?)?;
        define(bridge, &mut function, "description", string(bridge, &tool.description)?)?;
        let parameters = bridge.tool_wire_schema(tool)?;
        define(bridge, &mut function, "parameters", parameters)?;
        define(bridge, &mut declaration, "function", function)?;
        // Native Array.join represents an undefined mapped element as empty.
        lines.push(bridge.stringify_plain(&declaration)?.unwrap_or_default());
    }
    Ok(lines.join("\n"))
}

pub fn render_inband_tool_prompt<F: ToolCatalogBridge, D: DialectRenderer>(
    tools: &[crate::Tool],
    definition: &D,
    bridge: &F,
) -> Result<String, F::Error> {
    let prompt = ara_prompt::js::trim(definition.prompt());
    let catalog = render_tool_catalog(tools, bridge)?;
    // Native replace callbacks replace the first occurrence only and preserve
    // literal replacement text. The second replacement runs on the first's
    // result, including an authored token in a tool description if present.
    Ok(include_str!("prompt-template.md").replacen("{{TOOLS}}", &catalog, 1).replacen("{{DIALECT}}", prompt, 1))
}
