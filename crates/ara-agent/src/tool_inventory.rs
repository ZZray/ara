//! Display-only tool catalogue, ported from fixed OMP
//! `packages/ai/src/{dialect/inventory,utils/schema/typescript}.ts` at
//! 596f2da7101178214aa27a753529d15e6b7ad91d (MIT; THIRD_PARTY_NOTICES.md).
//! Refinement constraints intentionally stay out of the displayed type.

use ara_ai::Tool;
use ara_prompt::js;
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

use crate::provider_context::{ProviderContextError, render_tool_examples_jsdoc};

/// Native display options: defaults use JSDoc, semicolons and two spaces;
/// `harmony` uses line comments, commas and no indentation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JsonSchemaToTsOptions {
    pub indent: Option<String>,
    pub comments: Option<bool>,
    pub style: Option<String>,
}

struct DisplayContext<'a> {
    indent: &'a str,
    comments: bool,
    harmony: bool,
    defs: HashMap<&'a str, &'a Value>,
    seen: HashSet<&'a str>,
}

fn literal(value: &Value) -> String {
    js::json_stringify(value)
}

fn join_union(parts: impl IntoIterator<Item = String>) -> String {
    let mut seen = HashSet::new();
    let parts: Vec<String> = parts.into_iter().filter(|part| seen.insert(part.clone())).collect();
    if parts.is_empty() { "never".into() } else { parts.join(" | ") }
}

fn emit_description(lines: &mut Vec<String>, description: &str, ctx: &DisplayContext<'_>, pad: &str) {
    if ctx.harmony {
        for line in description.split('\n') {
            lines.push(js::trim_end(&format!("{pad}// {line}")).to_string());
        }
    } else {
        let safe = description.replace("*/", "* /");
        if !safe.contains('\n') {
            lines.push(format!("{pad}/** {safe} */"));
        } else {
            lines.push(format!("{pad}/**"));
            lines.extend(safe.split('\n').map(|line| format!("{pad} * {line}")));
            lines.push(format!("{pad} */"));
        }
    }
}

fn convert_array(node: &Map<String, Value>, ctx: &mut DisplayContext<'_>, pad: &str) -> String {
    if let Some(prefix) = node.get("prefixItems").and_then(Value::as_array) {
        return format!("[{}]", prefix.iter().map(|item| convert(item, ctx, pad)).collect::<Vec<_>>().join(", "));
    }
    match node.get("items") {
        None | Some(Value::Bool(true)) => "unknown[]".into(),
        Some(Value::Bool(false)) => "never[]".into(),
        Some(items) => {
            let inner = convert(items, ctx, pad);
            if inner.contains('\n') || inner.contains(" | ") || inner.encode_utf16().count() > 40 {
                format!("Array<{inner}>")
            } else {
                format!("{inner}[]")
            }
        }
    }
}

fn safe_key(key: &str) -> bool {
    let mut chars = key.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

fn convert_object(node: &Map<String, Value>, ctx: &mut DisplayContext<'_>, pad: &str) -> String {
    let child_pad = format!("{pad}{}", ctx.indent);
    let delimiter = if ctx.harmony { ',' } else { ';' };
    let mut body = Vec::new();
    if let Some(properties) = node.get("properties").and_then(Value::as_object) {
        let required: HashSet<&str> =
            node.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
        for (key, value) in js::entries(properties) {
            if ctx.comments
                && let Some(description) = value.as_object().and_then(|v| v.get("description")).and_then(Value::as_str)
                && !description.is_empty()
            {
                emit_description(&mut body, description, ctx, &child_pad);
            }
            let optional = if required.contains(key.as_str()) { "" } else { "?" };
            let name = if safe_key(key) { key.clone() } else { literal(&Value::String(key.clone())) };
            let rendered = convert(value, ctx, &child_pad);
            body.push(format!("{child_pad}{name}{optional}: {rendered}{delimiter}"));
        }
    }
    let additional = node.get("additionalProperties");
    if body.is_empty() {
        return match additional {
            Some(value @ Value::Object(_)) => format!("Record<string, {}>", convert(value, ctx, pad)),
            Some(Value::Bool(true)) => "Record<string, unknown>".into(),
            _ => "{}".into(),
        };
    }
    if let Some(value @ Value::Object(_)) = additional {
        body.push(format!("{child_pad}[key: string]: {}{delimiter}", convert(value, ctx, &child_pad)));
    }
    format!("{{\n{}\n{pad}}}", body.join("\n"))
}

fn convert_type(kind: &str, node: &Map<String, Value>, ctx: &mut DisplayContext<'_>, pad: &str) -> String {
    match kind {
        "string" => "string".into(),
        "integer" | "number" => "number".into(),
        "boolean" => "boolean".into(),
        "null" => "null".into(),
        "array" => convert_array(node, ctx, pad),
        "object" => convert_object(node, ctx, pad),
        _ => "unknown".into(),
    }
}

fn convert(node: &Value, ctx: &mut DisplayContext<'_>, pad: &str) -> String {
    let node = match node {
        Value::Bool(false) => return "never".into(),
        Value::Object(node) => node,
        _ => return "unknown".into(),
    };
    if let Some(reference) = node.get("$ref").and_then(Value::as_str) {
        let key = reference.strip_prefix("#/$defs/").or_else(|| reference.strip_prefix("#/definitions/"));
        if let Some(key) = key.filter(|key| !key.is_empty())
            && let Some((&definition_key, &resolved)) = ctx.defs.get_key_value(key)
            && resolved.is_object()
            && ctx.seen.insert(definition_key)
        {
            let out = convert(resolved, ctx, pad);
            ctx.seen.remove(definition_key);
            return out;
        }
        return reference.rsplit('/').next().unwrap_or(reference).to_string();
    }
    if let Some(value) = node.get("const") {
        return literal(value);
    }
    if let Some(values) = node.get("enum").and_then(Value::as_array) {
        return join_union(values.iter().map(literal));
    }
    if let Some(variants) =
        node.get("anyOf").and_then(Value::as_array).or_else(|| node.get("oneOf").and_then(Value::as_array))
    {
        return join_union(variants.iter().map(|variant| convert(variant, ctx, pad)));
    }
    if let Some(variants) = node.get("allOf").and_then(Value::as_array) {
        return variants.iter().map(|variant| convert(variant, ctx, pad)).collect::<Vec<_>>().join(" & ");
    }
    match node.get("type") {
        Some(Value::Array(kinds)) => join_union(kinds.iter().map(|kind| {
            // Here JSON null is an actual JS null, not the template helper's undefined sentinel.
            let kind = if kind.is_null() { "null".to_string() } else { js::to_string(kind) };
            convert_type(&kind, node, ctx, pad)
        })),
        Some(Value::String(kind)) => convert_type(kind, node, ctx, pad),
        _ => "unknown".into(),
    }
}

/// Render the Native simplified display type, preserving JavaScript key and union order.
pub fn json_schema_to_typescript(schema: &Value, options: Option<&JsonSchemaToTsOptions>) -> String {
    let harmony = options.and_then(|o| o.style.as_deref()) == Some("harmony");
    let mut ctx = DisplayContext {
        indent: options.and_then(|o| o.indent.as_deref()).unwrap_or(if harmony { "" } else { "  " }),
        comments: options.and_then(|o| o.comments).unwrap_or(true),
        harmony,
        defs: HashMap::new(),
        seen: HashSet::new(),
    };
    if let Some(root) = schema.as_object() {
        for key in ["definitions", "$defs"] {
            if let Some(definitions) = root.get(key).and_then(Value::as_object) {
                for (key, value) in definitions {
                    ctx.defs.insert(key, value);
                }
            }
        }
    }
    convert(schema, &mut ctx, "")
}

/// Render raw tool metadata once; normalizer output would duplicate examples and intent.
pub fn render_tool_inventory(tools: &[Tool]) -> Result<String, ProviderContextError> {
    if tools.is_empty() {
        return Ok(String::new());
    }
    let mut declarations = Vec::with_capacity(tools.len());
    let options = JsonSchemaToTsOptions { style: Some("harmony".into()), ..Default::default() };
    for tool in tools {
        let schema = ara_ai::tool_wire_schema(&tool.parameters)
            .map_err(|error| ProviderContextError::Schema(error.to_string()))?;
        let params = json_schema_to_typescript(&schema, Some(&options));
        let mut lines = Vec::new();
        if !tool.description.is_empty() {
            lines.extend(tool.description.split('\n').map(|line| js::trim_end(&format!("// {line}")).to_string()));
        }
        let examples = render_tool_examples_jsdoc(tool);
        if !examples.is_empty() {
            if !tool.description.is_empty() {
                lines.push("//".into());
            }
            lines.extend(examples.split('\n').map(|line| js::trim_end(&format!("// {line}")).to_string()));
        }
        lines.push(if params == "{}" {
            format!("type {} = ();", tool.name)
        } else {
            format!("type {} = (_: {params});", tool.name)
        });
        declarations.push(lines.join("\n"));
    }
    Ok(format!("## functions\n\nnamespace functions {{\n\n{}\n\n}} // namespace functions", declarations.join("\n\n")))
}
