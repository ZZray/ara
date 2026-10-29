//! Structured external-effect status of tool results (requirement 22).
//!
//! `reports_unknown` is the one structured test shared by Core, Harness and
//! Hosts. `classify_builtin` is Core's adapter for the error contracts of its
//! own builtin effect tools, so no later layer needs to match error text.
use serde_json::{json, Value};

/// Structured markers that declare unknown external effects.
pub fn reports_unknown(details: &Value) -> bool {
    match details {
        Value::Array(items) => items.iter().any(reports_unknown),
        Value::Object(object) => {
            object.get("executed").and_then(Value::as_str) == Some("unknown")
                || object.get("panicked").and_then(Value::as_bool) == Some(true)
                || object.get("effects_unknown").and_then(Value::as_bool) == Some(true)
                || object.get("source").and_then(Value::as_str) == Some("interrupted_unknown_effect")
                || object.get("status").and_then(Value::as_str) == Some("effects_unknown")
                || object.values().any(reports_unknown)
        }
        _ => false,
    }
}

/// Builtin tools whose failures may leave external effects behind.
pub(crate) const EFFECT_TOOLS: [&str; 8] = [
    "edit_file",
    "write_file",
    "exec",
    "run_script",
    "job_start",
    "job_write_input",
    "job_terminal_signal",
    "job_cancel",
];

/// First-line diagnostics of those tools that mean "effects unknown".
const PHRASES: [(&str, &str); 4] = [
    ("effects_unknown:", "effects_unknown_prefix"),
    ("partial effects may remain", "partial_effects_may_remain"),
    ("effects may be unknown", "effects_may_be_unknown"),
    ("do not replay the command without reconciling its effects", "do_not_replay"),
];

/// Structured status for a failed builtin effect tool whose error text states
/// unknown effects; `None` otherwise. Only the first line is Core's own
/// diagnostic (process output follows it), except a whole-JSON receipt.
pub(crate) fn classify_builtin(tool: &str, is_error: bool, text: &str) -> Option<Value> {
    if !is_error || !EFFECT_TOOLS.contains(&tool) {
        return None;
    }
    let first = text.lines().next().unwrap_or("");
    let marker =
        PHRASES.iter().find(|(phrase, _)| first.contains(phrase)).map(|(_, id)| *id).or_else(|| {
            serde_json::from_str::<Value>(text).ok().filter(reports_unknown).map(|_| "structured_receipt")
        })?;
    Some(json!({"effects_unknown": true, "source": "core_builtin_error_contract", "marker": marker}))
}

/// Merge `status` into existing tool result details without dropping them.
pub(crate) fn merge(details: Option<Value>, status: Value) -> Value {
    match details {
        Some(Value::Object(mut object)) => {
            if let Value::Object(extra) = status {
                object.extend(extra);
            }
            Value::Object(object)
        }
        None | Some(Value::Null) => status,
        Some(original) => {
            let mut wrapped = json!({ "original": original });
            if let (Value::Object(target), Value::Object(extra)) = (&mut wrapped, status) {
                target.extend(extra);
            }
            wrapped
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_phrases_on_the_first_line_of_effect_tool_errors_are_typed() {
        for (phrase, marker) in PHRASES {
            let text = format!("tool failed: {phrase} here\nstdout: effects_unknown: noise");
            let status = classify_builtin("exec", true, &text).unwrap();
            assert_eq!(status["marker"], marker);
            assert!(reports_unknown(&status));
        }
        let receipt = r#"{"status":"effects_unknown","detail":"x"}"#;
        assert_eq!(classify_builtin("write_file", true, receipt).unwrap()["marker"], "structured_receipt");
    }

    #[test]
    fn other_tools_successes_and_later_lines_stay_unclassified() {
        assert!(classify_builtin("read_file", true, "effects_unknown: x").is_none());
        assert!(classify_builtin("exec", false, "effects_unknown: x").is_none());
        assert!(classify_builtin("exec", true, "exit 1\neffects_unknown: printed by the program").is_none());
        assert!(classify_builtin("custom", true, r#"{"executed":"unknown"}"#).is_none());
    }

    #[test]
    fn merge_keeps_existing_details() {
        let status = json!({"effects_unknown": true});
        assert_eq!(merge(Some(json!({"a": 1})), status.clone()), json!({"a": 1, "effects_unknown": true}));
        assert_eq!(merge(Some(json!([1])), status.clone()), json!({"original": [1], "effects_unknown": true}));
        assert_eq!(merge(None, status.clone()), status);
    }
}
