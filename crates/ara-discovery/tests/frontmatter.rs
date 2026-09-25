//! Ports of OMP `packages/utils/test/frontmatter.test.ts` and the
//! `parseFrontmatter` cases of `packages/coding-agent/test/discovery/helpers.test.ts`
//! at 596f2da7101178214aa27a753529d15e6b7ad91d.

use ara_discovery::frontmatter::{FailureLevel, FrontmatterOptions, parse_frontmatter};
use serde_json::{Value, json};

fn parse(content: &str) -> (Value, String) {
    let options =
        FrontmatterOptions { source: Some("tests:frontmatter".into()), level: FailureLevel::Off, ..Default::default() };
    let result = parse_frontmatter(content, &options).unwrap();
    (Value::Object(result.frontmatter), result.body)
}

/// B-3747d5ce38, B-7b8fe3ff71, B-d0d5354897, B-f77902fa10
#[test]
fn parses_scalars_lists_blocks_and_nesting() {
    assert_eq!(
        parse("---\nname: test\nenabled: true\n---\nBody content"),
        (json!({"name": "test", "enabled": true}), "Body content".into())
    );
    assert_eq!(
        parse("---\ntags:\n  - javascript\n  - typescript\n  - react\n---\nBody content").0,
        json!({"tags": ["javascript", "typescript", "react"]})
    );
    assert_eq!(
        parse(
            "---\ndescription: |\n  This is a multi-line\n  description block\n  with several lines\n---\nBody content"
        )
        .0,
        json!({"description": "This is a multi-line\ndescription block\nwith several lines\n"})
    );
    assert_eq!(
        parse("---\nconfig:\n  server:\n    port: 3000\n    host: localhost\n  database:\n    name: mydb\n---\nBody content").0,
        json!({"config": {"server": {"port": 3000, "host": "localhost"}, "database": {"name": "mydb"}}})
    );
}

/// B-d6d5ad1caf, B-2b23751a48, B-58d5db2f71, B-8436cc5a24
#[test]
fn mixed_missing_empty_and_kebab_keys() {
    let mixed = "---\nname: complex-test\nversion: 1.0.0\ntags:\n  - prod\n  - critical\nmetadata:\n  author: tester\n  created: 2024-01-01\ndescription: |\n  Multi-line description\n  with formatting\n---\nBody content";
    assert_eq!(
        parse(mixed).0,
        json!({
            "name": "complex-test", "version": "1.0.0", "tags": ["prod", "critical"],
            "metadata": {"author": "tester", "created": "2024-01-01"},
            "description": "Multi-line description\nwith formatting\n",
        })
    );
    assert_eq!(parse("Just body content"), (json!({}), "Just body content".into()));
    assert_eq!(parse("---\n---\nBody content"), (json!({}), "Body content".into()));
    assert_eq!(
        parse("---\nthinking-level: medium\noutput-schema: json\nnested-field:\n  inner-key: value\n---\nBody content")
            .0,
        json!({"thinkingLevel": "medium", "outputSchema": "json", "nestedField": {"innerKey": "value"}})
    );
}

/// B-039c171679, B-300e5799e1
#[test]
fn unrecoverable_yaml_falls_back_with_a_warning() {
    assert_eq!(
        parse("---\ninvalid: [unclosed array\n---\nBody content"),
        (json!({"invalid": "[unclosed array"}), "Body content".into())
    );
    let options = FrontmatterOptions { source: Some("broken.md".into()), ..Default::default() };
    let result = parse_frontmatter("---\ninvalid: [unclosed array\n---\nBody content", &options).unwrap();
    assert_eq!(Value::Object(result.frontmatter), json!({"invalid": "[unclosed array"}));
    assert!(result.warning.unwrap().starts_with("Failed to parse YAML frontmatter (broken.md): "));
    let fatal = FrontmatterOptions { level: FailureLevel::Fatal, ..options };
    assert!(parse_frontmatter("---\ninvalid: [unclosed array\n---\nx", &fatal).is_err());
}

/// B-6b6eb375df
#[test]
fn colon_space_descriptions_are_repaired_without_warning() {
    let content = "---\nname: tool-prompt-optimization\ndescription: Optimize tool prompts. Two halves: measure schema overlap; keep scar tissue.\nenabled: true\n---\nSkill body";
    let options = FrontmatterOptions { source: Some("bad-skill/SKILL.md".into()), ..Default::default() };
    let result = parse_frontmatter(content, &options).unwrap();
    assert_eq!(
        Value::Object(result.frontmatter),
        json!({
            "name": "tool-prompt-optimization",
            "description": "Optimize tool prompts. Two halves: measure schema overlap; keep scar tissue.",
            "enabled": true,
        })
    );
    assert_eq!(result.body, "Skill body");
    assert_eq!(result.warning, None);
}

/// B-b01eef77cd
#[test]
fn fallback_reparses_each_value() {
    let content = "---\ncondition: \"(?i)pre.existing\"\nscope: \"text\",\"thinking\"\nenabled: true\n---\nBody";
    let options = FrontmatterOptions { source: Some("rule.md".into()), ..Default::default() };
    let result = parse_frontmatter(content, &options).unwrap();
    assert_eq!(result.frontmatter["condition"], "(?i)pre.existing");
    assert_eq!(result.frontmatter["enabled"], true);
    assert_eq!(result.frontmatter["scope"], "\"text\",\"thinking\"");
    assert_eq!(result.body, "Body");
    assert!(result.warning.is_some());
}

#[test]
fn crlf_html_comments_and_tabs_are_repaired() {
    let content = "<!-- lead -->---\r\nname: x\r\n\tdescription: y\r\n---\r\nbody <!-- hidden --> text\r\n";
    let (fm, body) = parse(content);
    assert_eq!(fm["name"], "x");
    assert_eq!(body, "body  text");
    let strict = FrontmatterOptions { repair: false, level: FailureLevel::Off, ..Default::default() };
    let result =
        parse_frontmatter("---\nkebab-key: 1\n---\nb", &FrontmatterOptions { raw_keys: true, ..strict }).unwrap();
    assert_eq!(result.frontmatter["kebab-key"], 1);
}

/// Review F1: deep nesting is an error handled by the fallback, not a stack
/// overflow (checked on a 2 MiB thread).
#[test]
fn deep_yaml_nesting_does_not_overflow() {
    let result = std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(|| {
            let content = format!("---\nname: x\ndescription: d\na:\n{}x\n---\nbody", "- ".repeat(30_000));
            let parsed =
                parse_frontmatter(&content, &FrontmatterOptions { level: FailureLevel::Off, ..Default::default() })
                    .unwrap();
            (Value::Object(parsed.frontmatter), parsed.body)
        })
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(result.0["name"], "x");
    assert_eq!(result.0["description"], "d");
    assert_eq!(result.1, "body");
    assert!(ara_discovery::frontmatter::parse_yaml(&format!("{}x", "- ".repeat(300))).is_err());
}

/// Review F3, F6, F7, F8: JS regex classes, `<<` scalars, key order, truncation.
#[test]
fn review_parity_details() {
    // ASCII `\w`: a non-ASCII key is not a fallback line.
    let (fm, _) = parse("---\ncafé: a: b\nbad: [\nok: 1\n---\nx");
    assert_eq!(fm, json!({"bad": "[", "ok": 1}));
    // `<<` with a scalar is an ordinary key.
    assert_eq!(parse("---\n<<: 5\n---\n").0, json!({"<<": 5}));
    // Integer-like keys first.
    let (fm, _) = parse("---\n2: two\nname: n\n1: one\n---\n");
    assert_eq!(fm.as_object().unwrap().keys().collect::<Vec<_>>(), ["1", "2", "name"]);
    // Inline source text: 63 UTF-16 units and an ellipsis.
    let content = format!("---\ninvalid: [x\n---\n{}", "y".repeat(80));
    let warning = parse_frontmatter(&content, &FrontmatterOptions::default()).unwrap().warning.unwrap();
    let inline = warning.split("(Inline '").nth(1).unwrap().split("'):").next().unwrap();
    assert_eq!(inline.encode_utf16().count(), 64);
    assert!(inline.ends_with('…'));
}
