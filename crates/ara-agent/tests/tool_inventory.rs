//! Module cases for fixed OMP json-schema-typescript and inventory rendering.
//! Source: 596f2da7101178214aa27a753529d15e6b7ad91d, MIT.
use ara_agent::{JsonSchemaToTsOptions, json_schema_to_typescript, render_tool_inventory};
use ara_ai::{Tool, ToolExample};
use serde_json::{Value, json};

#[test]
fn native_display_schema_shapes_and_styles() {
    let schema = json!({
        "type":"object", "properties":{
            "query":{"type":"string","description":"search query"},
            "limit":{"type":"integer","description":"max results"}
        },"required":["query"]
    });
    assert_eq!(
        json_schema_to_typescript(&schema, None),
        "{\n  /** search query */\n  query: string;\n  /** max results */\n  limit?: number;\n}"
    );
    let harmony = JsonSchemaToTsOptions { style: Some("harmony".into()), ..Default::default() };
    assert_eq!(
        json_schema_to_typescript(&schema, Some(&harmony)),
        "{\n// search query\nquery: string,\n// max results\nlimit?: number,\n}"
    );
    for (schema, expected) in [
        (json!(true), "unknown"),
        (json!(false), "never"),
        (json!({"enum":["day","week","day",null]}), "\"day\" | \"week\" | null"),
        (json!({"enum":[]}), "never"),
        (json!({"const":{"n":1}}), "{\"n\":1}"),
        (json!({"type":["integer","number","null"]}), "number | null"),
        (json!({"anyOf":[{"type":"number"},{"type":"null"}]}), "number | null"),
        (json!({"oneOf":[{"type":"string"},{"type":"boolean"}]}), "string | boolean"),
        (json!({"allOf":[{"type":"number"},{"const":4}]}), "number & 4"),
        (json!({"type":"array","items":{"type":"string"}}), "string[]"),
        (json!({"type":"array","prefixItems":[{"type":"string"},{"type":"number"}]}), "[string, number]"),
        (json!({"type":"object","additionalProperties":{"type":"number"}}), "Record<string, number>"),
        (json!({"type":"object","properties":{},"additionalProperties":false}), "{}"),
    ] {
        assert_eq!(json_schema_to_typescript(&schema, None), expected, "{schema}");
    }
}

#[test]
fn ordered_keys_refs_and_nested_descriptions_keep_native_display_contract() {
    let schema:Value=serde_json::from_str(r##"{"type":"object","properties":{"z":{"type":"string","description":"one */\ntwo"},"10":{"type":"number"},"2":{"$ref":"#/$defs/Node"},"not-safe":{"type":"array","items":{"type":"object","properties":{"id":{"type":"string"}}}}},"required":["z"],"definitions":{"Node":{"type":"boolean"}},"$defs":{"Node":{"type":"object","properties":{"next":{"$ref":"#/$defs/Node"}}}}}"##).unwrap();
    let text = json_schema_to_typescript(&schema, None);
    assert!(text.find("\"2\"?").unwrap() < text.find("\"10\"?").unwrap());
    assert!(text.find("\"10\"?").unwrap() < text.find("z:").unwrap());
    assert!(text.contains("next?: Node;"));
    assert!(text.contains(" * one * /\n   * two"));
    assert!(text.contains("\"not-safe\"?: Array<{\n"));
    assert!(!text.contains("boolean"));
    let no_comments = JsonSchemaToTsOptions { indent: Some("\t".into()), comments: Some(false), ..Default::default() };
    let text = json_schema_to_typescript(&schema, Some(&no_comments));
    assert!(!text.contains("/**"));
    assert!(text.contains("\n\tz: string;"));
    assert_eq!(json_schema_to_typescript(&json!({"$ref":"https://example.test/Missing"}), None), "Missing");
}

#[test]
fn raw_inventory_contains_full_metadata_once_and_leaves_authored_tools_unchanged() {
    let read = Tool {
        name: "read".into(),
        description: "Reads files from disk.\nSecond line  ".into(),
        parameters: json!({"type":"object","properties":{"path":{"type":"string","description":"file path"}},"required":["path"]}),
        examples: Some(vec![ToolExample::Call {
            caption: Some("Read source".into()),
            call: json!({"path":"src/main.rs","offset":1}).as_object().unwrap().clone(),
        }]),
        ..Default::default()
    };
    let tools = vec![
        read,
        Tool { name: "flush".into(), parameters: json!({"type":"object","properties":{}}), ..Default::default() },
    ];
    let original = serde_json::to_string(&tools).unwrap();
    let text = render_tool_inventory(&tools).unwrap();
    assert_eq!(
        text,
        "## functions\n\nnamespace functions {\n\n// Reads files from disk.\n// Second line\n//\n// @example \"Read source\"\n// read(path=\"src/main.rs\", offset=1)\ntype read = (_: {\n// file path\npath: string,\n});\n\ntype flush = ();\n\n} // namespace functions"
    );
    assert_eq!(serde_json::to_string(&tools).unwrap(), original);
    assert!(!text.contains("i="));
    assert_eq!(render_tool_inventory(&[]).unwrap(), "");
}
