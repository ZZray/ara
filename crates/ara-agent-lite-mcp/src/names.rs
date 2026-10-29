//! Tool names and input schemas (requirement 42.4), matching Claude Code
//! `strings.ts` naming and `discovery.ts` sanitizing.
use std::collections::BTreeSet;

use serde_json::{Map, Value};

/// Longest accepted full tool name.
pub const MAX_TOOL_NAME: usize = 128;
/// Claude Code `MAX_MCP_DESCRIPTION_LENGTH`.
pub const MAX_DESCRIPTION: usize = 2048;

/// Claude Code `normalizeNameForMCP`: every UTF-16 code unit outside
/// `[A-Za-z0-9_-]` becomes `_`.
pub fn normalize(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            out.push(c);
        } else {
            out.extend(std::iter::repeat_n('_', c.len_utf16()));
        }
    }
    out
}

/// `mcp__<server>__<tool>`; the tool name is sanitized first like Claude Code
/// sanitizes `tools/list` data before naming.
pub fn tool_name(server: &str, tool: &str) -> String {
    format!("mcp__{}__{}", normalize(server), normalize(&crate::text::sanitize(tool)))
}

/// Assigns full names in order. The first holder of a name keeps it; later
/// duplicates, empty names and names over 128 characters get a reason.
#[derive(Default)]
pub struct NameRegistry {
    taken: BTreeSet<String>,
}

impl NameRegistry {
    pub fn assign(&mut self, server: &str, tool: &str) -> Result<String, String> {
        if server.is_empty() || crate::text::sanitize(tool).is_empty() {
            return Err("empty server or tool name".into());
        }
        let full = tool_name(server, tool);
        if full.len() > MAX_TOOL_NAME {
            return Err(format!("full tool name exceeds {MAX_TOOL_NAME} characters"));
        }
        if !self.taken.insert(full.clone()) {
            return Err(format!("duplicate tool name {full} after normalization"));
        }
        Ok(full)
    }
}

/// A usable object schema plus the parameter-group note that goes before the
/// tool description when root-level `anyOf`/`oneOf`/`allOf` were merged.
pub fn flatten_schema(schema: &Value) -> Result<(Value, Option<String>), String> {
    let root = schema.as_object().ok_or("inputSchema is not an object")?;
    if root.get("type").is_some_and(|kind| kind != "object") {
        return Err("inputSchema type is not \"object\"".into());
    }
    let mut out = root.clone();
    out.insert("type".into(), "object".into());
    let combinators = ["allOf", "anyOf", "oneOf"];
    if !combinators.iter().any(|key| root.contains_key(*key)) {
        return Ok((Value::Object(out), None));
    }
    for key in combinators {
        out.remove(key);
    }
    let mut properties = root.get("properties").and_then(Value::as_object).cloned().unwrap_or_default();
    let mut required = names(root.get("required"));
    let mut notes = Vec::new();
    for key in combinators {
        let Some(branches) = root.get(key).and_then(Value::as_array) else { continue };
        let branches: Vec<&Map<String, Value>> = branches.iter().filter_map(Value::as_object).collect();
        let mut groups = Vec::new();
        for branch in &branches {
            if let Some(extra) = branch.get("properties").and_then(Value::as_object) {
                for (name, schema) in extra {
                    properties.entry(name.clone()).or_insert_with(|| schema.clone());
                }
            }
            groups.push(names(branch.get("required")));
        }
        let shared: Vec<String> = if key == "allOf" {
            groups.iter().flatten().cloned().collect()
        } else {
            groups
                .first()
                .map(|first| {
                    first.iter().filter(|name| groups.iter().all(|group| group.contains(name))).cloned().collect()
                })
                .unwrap_or_default()
        };
        for name in shared {
            if !required.contains(&name) {
                required.push(name);
            }
        }
        if key != "allOf" && groups.iter().any(|group| !group.is_empty()) {
            let rendered = groups
                .iter()
                .map(|group| if group.is_empty() { "(none)".to_string() } else { format!("[{}]", group.join(", ")) })
                .collect::<Vec<_>>()
                .join(" or ");
            let rule = if key == "oneOf" { "exactly one" } else { "at least one" };
            notes.push(format!("Provide {rule} of these parameter groups together: {rendered}."));
        }
    }
    out.insert("properties".into(), Value::Object(properties));
    if required.is_empty() {
        out.remove("required");
    } else {
        out.insert("required".into(), required.into_iter().map(Value::String).collect());
    }
    Ok((Value::Object(out), (!notes.is_empty()).then(|| notes.join(" "))))
}

fn names(value: Option<&Value>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for name in value.and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
        if !out.iter().any(|known| known == name) {
            out.push(name.to_string());
        }
    }
    out
}

/// Tool description: optional group note, then the server description cut
/// like Claude Code at 2048 characters.
pub fn description(note: Option<&str>, description: &str) -> String {
    let description = crate::text::sanitize(description);
    let cut = match description.char_indices().nth(MAX_DESCRIPTION) {
        Some((index, _)) => format!("{}… [truncated]", &description[..index]),
        None => description,
    };
    match note {
        Some(note) if cut.is_empty() => note.to_string(),
        Some(note) => format!("{note}\n\n{cut}"),
        None => cut,
    }
}
