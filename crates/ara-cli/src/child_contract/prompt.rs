//! Fixed b91db66e child prompt assets/helpers, MIT; THIRD_PARTY_NOTICES.md.
//! Copyright (c) 2025 Mario Zechner; (c) 2025-2026 Can Bölük; (c) 2026 Stencil Labs, Inc.
use super::{ChildContractError, WorkPoolYieldItem};
use ara_prompt::template::{CompileOptions, Engine, Output};
use serde_json::{Value, json};
pub const YIELD_DESCRIPTION: &str = include_str!("prompts/yield.md");
pub const CHILD_SYSTEM: &str = include_str!("prompts/subagent-system-prompt.md");
pub const YIELD_REMINDER: &str = include_str!("prompts/subagent-yield-reminder.md");
fn engine() -> Engine {
    let mut e = Engine::with_options(CompileOptions { no_escape: true });
    e.register_helper("renderYieldSchema", |call| {
        let ts = jtd_to_typescript(call.arg(0));
        let mut lines = ts.lines();
        let first = lines.next().unwrap_or("unknown");
        let rest = lines.map(|s| format!("  {s}")).collect::<Vec<_>>();
        let body = if rest.is_empty() {
            first.to_owned()
        } else {
            format!("{first}\n{}", rest.join("\n"))
        };
        Ok(Output::Value(json!(format!("{{\n  data: {body};\n}}"))))
    });
    e
}
pub fn yield_description(has_schema: bool, pooled: bool) -> Result<String, ChildContractError> {
    engine()
        .render(
            YIELD_DESCRIPTION,
            &json!({"hasOutputSchema":has_schema,"workPoolItems":pooled}),
        )
        .map_err(|e| ChildContractError(e.to_string()))
}
pub fn yield_reminder(
    retry_count: usize,
    max_retries: usize,
    structured: bool,
) -> Result<String, ChildContractError> {
    engine().render(YIELD_REMINDER,&json!({"retryCount":retry_count,"maxRetries":max_retries,"structured":structured,"budgetStop":false})).map_err(|e|ChildContractError(e.to_string()))
}
#[derive(Clone, Debug, Default)]
pub struct ChildPromptInputs {
    pub base: Vec<String>,
    pub agent: String,
    pub context: String,
    pub plan_reference: String,
    pub plan_reference_path: String,
    pub worktree: String,
    pub structured: bool,
    pub output_schema: Option<Value>,
    pub output_schema_overrides_agent: bool,
}
impl ChildPromptInputs {
    pub fn render(&self, items: &[WorkPoolYieldItem]) -> Result<Vec<String>, ChildContractError> {
        let body=engine().render(CHILD_SYSTEM,&json!({"agent":self.agent,"context":self.context,"planReference":self.plan_reference,"planReferencePath":self.plan_reference_path,"worktree":self.worktree,"structured":self.structured,"outputSchema":self.output_schema,"outputSchemaOverridesAgent":self.output_schema_overrides_agent,"workPoolYieldItems":items,"ircSelfId":"","ircPeers":[],"ircParkedCount":0,"ircOmittedCount":0})).map_err(|e|ChildContractError(e.to_string()))?;
        let mut blocks = self.base.clone();
        blocks.push(body);
        Ok(blocks)
    }
}
/// Initial-spawn/update helper. The caller retains this exact base in its
/// authored metadata. Cold restoration uses latest final blocks directly.
pub fn render_contract_prompt(
    base: &[String],
    items: &[WorkPoolYieldItem],
    structured: bool,
    output_schema: Option<&Value>,
) -> Result<Vec<String>, ChildContractError> {
    ChildPromptInputs {
        base: base.to_vec(),
        structured,
        output_schema: output_schema.cloned(),
        ..Default::default()
    }
    .render(items)
}
pub fn jtd_to_typescript(schema: &Value) -> String {
    fn convert(s: &Value, inline: bool) -> String {
        let Some(o) = s.as_object() else {
            return "unknown".into();
        };
        if o.is_empty() {
            return "unknown".into();
        }
        if let Some(t) = o.get("type").and_then(Value::as_str) {
            return match t {
                "boolean" => "boolean",
                "string" | "timestamp" => "string",
                "float32" | "float64" | "int8" | "uint8" | "int16" | "uint16" | "int32"
                | "uint32" => "number",
                _ => "unknown",
            }
            .into();
        }
        if let Some(a) = o.get("enum").and_then(Value::as_array) {
            return a
                .iter()
                .map(|v| format!("\"{}\"", v.as_str().unwrap_or("")))
                .collect::<Vec<_>>()
                .join(" | ");
        }
        if let Some(s) = o.get("elements") {
            let t = convert(s, true);
            return if t.contains('\n') || t.len() > 40 {
                format!("Array<{t}>")
            } else {
                format!("{t}[]")
            };
        }
        if let Some(s) = o.get("values") {
            return format!("Record<string, {}>", convert(s, true));
        }
        if o.contains_key("properties") || o.contains_key("optionalProperties") {
            let mut props = Vec::new();
            for key in ["properties", "optionalProperties"] {
                if let Some(m) = o.get(key).and_then(Value::as_object) {
                    for (name, s) in m {
                        let mut chars = name.chars();
                        let safe = chars
                            .next()
                            .is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, '_' | '$'))
                            && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '$'));
                        let name = if safe {
                            name.clone()
                        } else {
                            format!("\"{name}\"")
                        };
                        props.push(format!(
                            "{name}{}: {};",
                            if key == "optionalProperties" { "?" } else { "" },
                            convert(s, true)
                        ));
                    }
                }
            }
            if inline && props.len() <= 2 && props.join(" ").len() < 60 {
                return format!("{{ {} }}", props.join(" "));
            }
            return format!(
                "{{\n{}\n}}",
                props
                    .iter()
                    .map(|p| format!("  {p}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
        if let (Some(tag), Some(mapping)) = (
            o.get("discriminator").and_then(Value::as_str),
            o.get("mapping").and_then(Value::as_object),
        ) {
            return mapping
                .iter()
                .map(|(label, s)| {
                    let p = convert(s, true);
                    if p == "{}" {
                        format!("{{ {tag}: \"{label}\" }}")
                    } else {
                        let body = p
                            .strip_prefix('{')
                            .and_then(|s| s.strip_suffix('}'))
                            .unwrap_or(&p)
                            .trim();
                        format!("{{ {tag}: \"{label}\"; {body} }}")
                    }
                })
                .collect::<Vec<_>>()
                .join(" | ");
        }
        o.get("ref")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .into()
    }
    convert(schema, false)
}
