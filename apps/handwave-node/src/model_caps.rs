//! Directory declarations and a conservative text-byte budget, not token usage.
use anyhow::{bail, ensure, Context, Result};
use serde::Serialize;
use serde_json::Value;

const LOCAL_INPUT_BYTES: u64 = 4 * 1024 * 1024;
const WIRE_RESERVE: u64 = 4096;

#[derive(Clone, Debug, Serialize)]
pub struct ModelCaps {
    pub context_window: Option<u64>,
    pub max_tokens: u64,
    pub input: Option<Vec<String>>,
}

fn positive(row: &Value, name: &str, nested: &str) -> Result<Option<u64>> {
    let read = |value: Option<&Value>| -> Result<Option<u64>> {
        match value {
            None | Some(Value::Null) => Ok(None),
            Some(v) => Ok(Some(v.as_u64().context("model capacity must be a nonnegative integer")?).filter(|n| *n > 0)),
        }
    };
    let top = read(row.get(name))?;
    let omp = read(row.get("omp").and_then(|v| v.get(nested)))?;
    // Conflicting declarations must never enlarge the safe candidate envelope.
    Ok(match (top, omp) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    })
}

impl ModelCaps {
    pub fn parse(row: &Value) -> Result<Self> {
        let parse_input = |value: Option<&Value>| -> Result<Option<Vec<String>>> {
            let Some(value) = value.filter(|v| !v.is_null()) else {
                return Ok(None);
            };
            let array = value.as_array().context("model input modalities must be an array")?;
            ensure!(array.len() <= 32, "too many model input modalities");
            let mut out = Vec::new();
            for entry in array {
                let text = entry.as_str().context("model input modality must be a string")?;
                ensure!(!text.is_empty() && text.len() <= 64 && text.trim() == text, "invalid model input modality");
                if !out.iter().any(|v| v == text) {
                    out.push(text.to_owned());
                }
            }
            Ok(Some(out))
        };
        let top = parse_input(row.get("input"))?;
        let omp = parse_input(row.get("omp").and_then(|v| v.get("input")))?;
        let input = match (top, omp) {
            (Some(a), Some(b)) => Some(a.into_iter().filter(|x| b.contains(x)).collect()),
            (a, b) => a.or(b),
        };
        Ok(Self {
            context_window: positive(row, "context_window", "contextWindow")?,
            max_tokens: positive(row, "max_tokens", "maxTokens")?.unwrap_or(8192),
            input,
        })
    }

    pub fn budget(&self, output_tokens: u64) -> Result<TextBudget> {
        if let Some(input) = &self.input {
            ensure!(input.iter().any(|m| m == "text"), "selected model does not declare compatible text input");
        }
        match self.context_window {
            None => Ok(TextBudget {
                projection_bytes: crate::context_memory::CONTEXT_LIMIT,
                request_bytes: crate::context_memory::CONTEXT_LIMIT + 64 * 1024,
            }),
            Some(window) => {
                let available = window
                    .checked_sub(output_tokens)
                    .and_then(|n| n.checked_sub(WIRE_RESERVE))
                    .context("model context is too small for output and request reserves")?
                    .min(LOCAL_INPUT_BYTES);
                ensure!(available >= 1024, "model has insufficient input context after reserves");
                let overhead = (64 * 1024).min(available / 2);
                Ok(TextBudget { projection_bytes: (available - overhead) as usize, request_bytes: available as usize })
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct TextBudget {
    pub projection_bytes: usize,
    pub request_bytes: usize,
}
impl TextBudget {
    pub fn check(&self, context: &rpi_ai::Context) -> Result<()> {
        // Includes actual system prompt, schemas and escaped message strings.
        // UTF-8 JSON bytes are a conservative text allocation, not a tokenizer
        // or a guarantee about undocumented supplier framing/image accounting.
        if serde_json::to_vec(context)?.len() > self.request_bytes {
            bail!("complete model request exceeds declared context/local text-byte budget; original Session retained");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn directory_capabilities_are_conservative_and_strict() {
        let c =
            ModelCaps::parse(&json!({"context_window":1_000_000,"max_tokens":16384,"input":["text","image"]})).unwrap();
        assert_eq!(c.context_window, Some(1_000_000));
        assert_eq!(c.input.unwrap(), vec!["text", "image"]);
        let c = ModelCaps::parse(&json!({"context_window":1_000_000,"omp":{"contextWindow":32768,"maxTokens":4096,"input":["text"]},"input":["text","image"]})).unwrap();
        assert_eq!(c.context_window, Some(32768));
        assert_eq!(c.max_tokens, 4096);
        assert_eq!(c.input.unwrap(), vec!["text"]);
        for row in [
            json!({"context_window":-1}),
            json!({"context_window":1.5}),
            json!({"max_tokens":"16384"}),
            json!({"input":"image"}),
            json!({"input":[7]}),
            json!({"omp":{"contextWindow":"1M"}}),
        ] {
            assert!(ModelCaps::parse(&row).is_err());
        }
    }
    #[test]
    fn text_budget_respects_capacity_and_local_resource_limit() {
        let b = |n| ModelCaps::parse(&json!({"context_window":n})).unwrap().budget(8192);
        assert_eq!(b(0).unwrap().projection_bytes, crate::context_memory::CONTEXT_LIMIT);
        assert!(b(1_000_000).unwrap().projection_bytes > crate::context_memory::CONTEXT_LIMIT);
        assert!(b(32768).unwrap().projection_bytes < crate::context_memory::CONTEXT_LIMIT);
        assert!(b(4096).is_err());
        assert_eq!(b(u64::MAX).unwrap().request_bytes, LOCAL_INPUT_BYTES as usize);
    }
    #[test]
    fn full_context_checks_system_schemas_and_json_escaping() {
        let budget = TextBudget { projection_bytes: 512, request_bytes: 1024 };
        let context = rpi_ai::Context { system_prompt: Some("x".repeat(1100)), ..Default::default() };
        assert!(budget.check(&context).is_err());
        let context = rpi_ai::Context::new(vec![rpi_ai::Message::User(rpi_ai::UserMessage {
            role: Default::default(),
            content: "\\\"".repeat(400).into(),
            timestamp: 0,
        })]);
        assert!(budget.check(&context).is_err());
    }
}
