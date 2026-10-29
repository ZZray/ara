//! Property 46: MCP tool name normalization (requirement 42.4).
use ara_agent_lite_mcp::names::{flatten_schema, normalize, tool_name, NameRegistry, MAX_TOOL_NAME};
use proptest::prelude::*;
use serde_json::json;

/// Independent reference of Claude Code `normalizeNameForMCP`, which runs a
/// regex over UTF-16 code units.
fn reference(name: &str) -> String {
    name.encode_utf16()
        .map(|unit| match char::from_u32(unit as u32) {
            Some(c) if c.is_ascii_alphanumeric() || c == '_' || c == '-' => c,
            _ => '_',
        })
        .collect()
}

fn strip_controls(name: &str) -> String {
    name.chars().filter(|c| !matches!(*c as u32, 0..=8 | 11 | 12 | 14..=31 | 0x7f | 0xfffd)).collect()
}

fn pattern(full: &str) -> bool {
    // ^mcp__[A-Za-z0-9_-]+__[A-Za-z0-9_-]+$
    let Some(rest) = full.strip_prefix("mcp__") else { return false };
    let allowed = rest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    allowed && (1..rest.len().saturating_sub(2)).any(|i| &rest[i..i + 2] == "__")
}

fn name_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-z]{1,8}",
        "[A-Za-z0-9_. -]{1,40}",
        any::<String>().prop_filter("non-empty", |s| !s.is_empty()),
        "[a-z]{60,90}",
        Just("dup.x".to_string()),
        Just("dup_x".to_string()),
        Just("名字😀".to_string()),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn normalize_matches_reference(name in any::<String>()) {
        prop_assert_eq!(normalize(&name), reference(&name));
    }

    #[test]
    fn names_match_pattern_or_are_excluded_and_never_collide(
        entries in proptest::collection::vec(("[a-z.]{1,6}", name_strategy()), 1..24)
    ) {
        let mut registry = NameRegistry::default();
        let mut seen = std::collections::BTreeSet::new();
        for (server, tool) in &entries {
            let expected = format!("mcp__{}__{}", reference(server), reference(&strip_controls(tool)));
            prop_assert_eq!(tool_name(server, tool), expected.clone());
            match registry.assign(server, tool) {
                Ok(full) => {
                    prop_assert_eq!(&full, &expected);
                    prop_assert!(pattern(&full), "{}", full);
                    prop_assert!(full.len() <= MAX_TOOL_NAME);
                    prop_assert!(seen.insert(full), "duplicate accepted");
                }
                Err(reason) => {
                    let empty = strip_controls(tool).is_empty();
                    let long = expected.len() > MAX_TOOL_NAME;
                    let duplicate = seen.contains(&expected);
                    prop_assert!(empty || long || duplicate, "{} {}", expected, reason);
                    if !empty && !long {
                        prop_assert!(reason.contains("duplicate"));
                    }
                }
            }
        }
    }
}

#[test]
fn root_combinators_merge_into_one_object() {
    let (schema, note) = flatten_schema(&json!({
        "type":"object",
        "properties":{"mode":{"type":"string"}},
        "required":["mode"],
        "anyOf":[{"properties":{"a":{"type":"string"}},"required":["a","shared"]},{"properties":{"b":{"type":"string"}},"required":["b","shared"]}],
        "allOf":[{"properties":{"shared":{"type":"string"}},"required":["shared"]}]
    }))
    .unwrap();
    assert_eq!(schema["required"], json!(["mode", "shared"]));
    assert!(schema.get("anyOf").is_none() && schema.get("allOf").is_none());
    assert_eq!(schema["properties"].as_object().unwrap().len(), 4);
    assert_eq!(note.unwrap(), "Provide at least one of these parameter groups together: [a, shared] or [b, shared].");
    assert_eq!(flatten_schema(&json!({"properties":{}})).unwrap(), (json!({"type":"object","properties":{}}), None));
    assert!(flatten_schema(&json!("x")).is_err());
    assert!(flatten_schema(&json!({"type":"array"})).is_err());
}
