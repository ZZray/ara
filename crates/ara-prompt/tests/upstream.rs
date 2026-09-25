//! Ports of OMP `packages/utils/test/{template,prompt}.test.ts` and
//! `packages/coding-agent/test/prompt-format.test.ts` at
//! 596f2da7101178214aa27a753529d15e6b7ad91d. Each test names the inventory
//! behavior IDs it covers (`docs/upstream/inventory/behaviors.tsv`).

use ara_prompt::{Engine, FormatOptions, Output, RenderPhase, format, render};
use serde_json::{Value, json};
use std::sync::Arc;

const FULL: FormatOptions =
    FormatOptions { render_phase: RenderPhase::PreRender, replace_ascii_symbols: true, normalize_rfc2119: true };
const PRE: FormatOptions = FormatOptions { render_phase: RenderPhase::PreRender, ..DEFAULT };
const POST: FormatOptions = DEFAULT;
const DEFAULT: FormatOptions =
    FormatOptions { render_phase: RenderPhase::PostRender, replace_ascii_symbols: false, normalize_rfc2119: false };
const PRE_ASCII: FormatOptions = FormatOptions { replace_ascii_symbols: true, ..PRE };

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!("{}/tests/fixtures/template/{name}.md", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

// --- template.test.ts ---------------------------------------------------------

/// B-4060eb9b2d: real template goldens captured from handlebars 4.7.9.
#[test]
fn real_template_goldens() {
    let goldens: [(&str, Value, &str); 5] = [
        (
            "session-user",
            json!({
                "user_context": "Keep API",
                "changelog_targets": "packages/x",
                "existing_changelog_entries": [{
                    "path": "CHANGELOG.md",
                    "sections": [{ "name": "Added", "items": ["A", "B"] }, { "name": "Fixed", "items": ["C"] }],
                }],
            }),
            "Generate conventional commit proposal for current staged changes.\n\nUser context:\nKeep API\n\nChangelog targets (must call propose_changelog for these files):\npackages/x\n\n## Existing Unreleased Changelog Entries\nMay include entries from list in propose_changelog `deletions` field for removal.\n### CHANGELOG.md\nAdded:\n- A\n- B\nFixed:\n- C\n\nUse git_* tools to inspect changes. Call analyze_files for deeper per-file summaries. Finish with propose_commit or split_commit.",
        ),
        (
            "frontmatter",
            json!({
                "name": "reviewer", "description": "Find bugs", "spawns": ["scout"], "model": "slow",
                "thinkingLevel": "high", "blocking": true, "prewalk": false, "autoloadSkills": ["react"], "body": "Body",
            }),
            "---\n\nname: \"reviewer\"\ndescription: \"Find bugs\"\nspawns: [\"scout\"]\nmodel: \"slow\"\nthinking-level: \"high\"\nblocking: true\nautoloadSkills: [\"react\"]\n---\nBody",
        ),
        (
            "identifier-task",
            json!({ "filename": "src/a.ts", "correct": "requestId", "misspelled": "reqeustId", "count": 2, "affectedLines": [3, 8] }),
            "# Fix a misspelled identifier in `src/a.ts`\n\nA recent edit misspelled the identifier `requestId` as `reqeustId` in 2 places.\n\nAffected lines: 3, 8.\n\nReplace every occurrence of `reqeustId` with `requestId`. Do not change anything else.",
        ),
        (
            "file-operations",
            json!({ "files": "read: a.ts\nmodified: b.ts" }),
            "<files>\nread: a.ts\nmodified: b.ts\n</files>",
        ),
        (
            "structural-task",
            json!({
                "filename": "src/a.ts", "kind": "swap-lines", "secondHead": "b();", "firstHead": "a();", "hunkCount": 2,
                "fence": "```", "language": "ts",
                "hunks": [{ "startLine": 4, "newCode": "a();\nb();" }, { "startLine": 0, "newCode": "c();" }],
            }),
            "# Fix a bug in `src/a.ts`\n\nTwo adjacent statements are in the wrong order: `b();` belongs before `a();`. Swap the two statements.\n\nAfter the fix, the affected regions must read exactly:\n\nAround line 4:\n\n```ts\na();\nb();\n```\n\n```ts\nc();\n```\n\nMake exactly this change; do not modify anything else.",
        ),
    ];
    for (name, context, expected) in goldens {
        assert_eq!(render(&fixture(name), &context).unwrap(), expected, "golden {name}");
    }
}

/// B-bf262bb93c
#[test]
fn resolves_dotted_bracket_this_parent_root_and_iteration_paths() {
    let engine = Engine::new();
    let source = "{{user.name}}/{{user.[display name]}}/{{[literal.key]}}|{{#each groups}}{{@index}}:{{#each items}}{{../name}}/{{@root.title}}/{{@index}}/{{@last}}={{this}};{{/each}}{{/each}}";
    let context = json!({
        "user": { "name": "Ada", "display name": "A" },
        "literal.key": "L",
        "title": "R",
        "groups": [{ "name": "G", "items": [0, false, ""] }],
    });
    assert_eq!(engine.render(source, &context).unwrap(), "Ada/A/L|0:G/R/0/false=0;G/R/1/false=false;G/R/2/true=;");
}

/// B-766a1cab46
#[test]
fn handlebars_falsy_rules_while_each_visits_falsy_entries() {
    let engine = Engine::new();
    let source = "{{#if zero}}bad{{else}}zero{{/if}}|{{#if empty}}bad{{else}}empty{{/if}}|{{#unless zero}}unless{{/unless}}|{{#each values}}[{{this}}]{{else}}none{{/each}}|{{#each object}}{{@key}}={{this}}/{{@last}};{{/each}}|{{#each missing}}bad{{else}}none{{/each}}";
    let context = json!({ "zero": 0, "empty": [], "values": [0, false, ""], "object": { "a": 1, "b": 2 } });
    assert_eq!(engine.render(source, &context).unwrap(), "zero|empty|unless|[0][false][]|a=1/false;b=2/true;|none");
}

/// B-029838c0c4
#[test]
fn hash_arguments_and_helper_subexpressions() {
    let mut engine = Engine::new();
    engine.register_helper("eq", |h| Ok(Output::from(h.arg(0) == h.arg(1))));
    engine.register_helper("label", |h| {
        let prefix = h.hash.get("prefix").and_then(Value::as_str).unwrap_or_default().to_string();
        Ok(Output::from(format!("{prefix}:{}", h.arg(0))))
    });
    let out = engine.render(r#"{{label (eq left right) prefix="same"}}"#, &json!({ "left": 3, "right": 3 })).unwrap();
    assert_eq!(out, "same:true");
}

/// B-e434e1349d
#[test]
fn block_helpers_get_fn_inverse_and_hash() {
    let mut engine = Engine::new();
    engine.register_helper("choose", |h| {
        let chosen = h.hash.get("expected") == Some(h.arg(0));
        Ok(Output::from(if chosen { h.render(h.this())? } else { h.inverse(h.this())? }))
    });
    let source = r#"{{#choose value expected="yes"}}Y{{else}}N{{/choose}}"#;
    assert_eq!(engine.render(source, &json!({ "value": "yes" })).unwrap(), "Y");
    assert_eq!(engine.render(source, &json!({ "value": "no" })).unwrap(), "N");
}

/// B-86a87bb762
#[test]
fn escapes_exact_entity_set_triple_stache_and_safe_string() {
    let mut engine = Engine::new();
    engine.register_helper("safe", |h| Ok(Output::Safe(h.arg(0).as_str().unwrap_or_default().to_string())));
    let value = "&<>\"'`=";
    let out = engine
        .render("{{! short}}{{!-- long --}}{{value}}|{{{value}}}|{{safe value}}", &json!({ "value": value }))
        .unwrap();
    assert_eq!(out, "&amp;&lt;&gt;&quot;&#x27;&#x60;&#x3D;|&<>\"'`=|&<>\"'`=");
    assert_eq!(engine.render("A\n{{!--\nlong\n--}}\nB", &json!({})).unwrap(), "A\nB");
}

/// B-e066ec7008
#[test]
fn helpers_win_over_properties_and_missing_paths_render_empty() {
    let mut engine = Engine::new();
    engine.register_helper("name", |_| Ok(Output::from("helper")));
    assert_eq!(engine.render("{{name}}/{{missing.path}}", &json!({ "name": "property" })).unwrap(), "helper/");
}

/// B-be72886afb: every upstream `packages/*/src/**/*.md` compiles. The full
/// corpus comparison lives in `oracle.rs` (`upstream_prompt_corpus_matches`);
/// here the five captured fixtures stand in when no OMP checkout is present.
#[test]
fn repository_templates_compile() {
    for name in ["session-user", "frontmatter", "identifier-task", "file-operations", "structural-task"] {
        ara_prompt::compile(&fixture(name)).unwrap();
    }
}

// --- prompt.test.ts ------------------------------------------------------------

/// B-74a324f00f, B-f23cea95fa, B-ff76021e62, B-7819602754, B-6f2ddd3e9f, B-8440d55e1c
#[test]
fn ascii_symbol_replacement() {
    assert_eq!(format("a -> b <- c <-> d != e <= f >= g ... h", FULL), "a → b ← c ↔ d ≠ e ≤ f ≥ g … h");
    assert_eq!(format("<=-> <-> ->= <-- -->x", FULL), "≤→ ↔ →= ←- -→x");
    assert_eq!(format("....... ..", FULL), "……. ..");
    assert_eq!(format("......", FULL), "……");
    assert_eq!(format("....", FULL), "….");
    assert_eq!(format("<!-- a -> b --> c -> d", FULL), "<!-- a -> b --> c → d");
    assert_eq!(format("<!--\nA -> B\n-->\nC -> D", FULL), "<!--\nA -> B\n-->\nC → D");
    assert_eq!(format("x --> y != z", FULL), "x -→ y ≠ z");
    let fenced = "```\na -> b\n```";
    assert_eq!(format(fenced, FULL), fenced);
}

/// B-b98c02b29e, B-60fc3779ea, B-fe413fcebe
#[test]
fn rfc2119_normalization() {
    assert_eq!(
        format("You **MUST** act. You **MUST NOT** stall. SHOULD NOT applies.", FULL),
        "You MUST act. You NEVER stall. AVOID applies."
    );
    assert_eq!(format("alias `MUST NOT` means MUST NOT", FULL), "alias `MUST NOT` means NEVER");
    assert_eq!(format("**bold** stays **bold**", FULL), "**bold** stays **bold**");
}

/// B-2c9e7a7ac3, B-1e047157c2, B-d3ccd5a49c, B-a269a36693, B-0863f53658, B-191e83daf0
#[test]
fn structure() {
    assert_eq!(format("| a | b |\n|:--- | --:|\n| c | d |", DEFAULT), "|a|b|\n|:---|---:|\n|c|d|");
    assert_eq!(format("  | a | b |", DEFAULT), "  |a|b|");
    assert_eq!(format("\n\na\n\n\nb\n \n\t\nc\n\n", DEFAULT), "a\nb\nc");
    assert_eq!(format("a\n\nb", DEFAULT), "a\n\nb");
    assert_eq!(format("<tag>\nbody\n\n</tag>", DEFAULT), "<tag>\nbody\n</tag>");
    assert_eq!(format("<a attr=\"x\">\nbody\n\n</a>", DEFAULT), "<a attr=\"x\">\nbody\n</a>");
    assert_eq!(format("<self/>\nx", DEFAULT), "<self/>\nx");
    let fenced = "```\na\n\n\n\nb\n```";
    assert_eq!(format(fenced, DEFAULT), fenced);
    assert_eq!(format("{{#if x}}\nbody\n\n{{/if}}", PRE), "{{#if x}}\nbody\n{{/if}}");
    assert_eq!(format("body\n\n{{/if}}", POST), "body\n\n{{/if}}");
}

/// B-8aa15fa3fa, B-58bd9c002c
#[test]
fn compile_cache_and_triple_braces() {
    let source = "Hello {{name}} {{#if x}}yes{{/if}}";
    assert!(Arc::ptr_eq(&ara_prompt::compile(source).unwrap(), &ara_prompt::compile(source).unwrap()));
    assert_eq!(render("{{#if a}}{ {{b}}}{{/if}}", &json!({ "a": true, "b": "v" })).unwrap(), "{ v}");
}

/// B-12ed310d46, B-00ecdb490e
#[test]
fn join_helper() {
    let files = json!({ "files": ["a.ts", "b.ts"] });
    assert_eq!(render(r#"{{join files "\n"}}"#, &files).unwrap(), "a.ts\nb.ts");
    assert_eq!(render(r#"{{join files "\t"}}"#, &files).unwrap(), "a.ts\tb.ts");
    assert_eq!(render("{{join files}}", &json!({ "files": ["a", "b"] })).unwrap(), "a, b");
    assert_eq!(render("{{join files}}", &json!({ "files": "not-an-array" })).unwrap(), "");
}

// --- coding-agent prompt-format.test.ts -----------------------------------------

/// B-4dcbc0b2f3, B-44f05064c8, B-d3e6fb07e9, B-9ca858cccc, B-86d36538e9
#[test]
fn render_phase_indentation_and_closers() {
    let block = "<root>\n  {{#if ok}}\n    value\n  {{/if}}\n</root>";
    assert_eq!(format(block, PRE), block);
    let tabs = "\t<root>\n\t  {{#if ok}}\n\t    value\n\t  {{/if}}\n</root>";
    assert_eq!(format(tabs, PRE), tabs);
    assert_eq!(
        format("\t<root>   \n\t  {{#if ok}}\t\n\t    value   \n\t  {{/if}} \n</root>", PRE),
        "\t<root>\n\t  {{#if ok}}\n\t    value\n\t  {{/if}}\n</root>"
    );
    assert_eq!(format(block, POST), block);
    let input = "<root>\n{{#if ok}}\nvalue\n\n{{/if}}\n</root>";
    assert_eq!(format(input, PRE), "<root>\n{{#if ok}}\nvalue\n{{/if}}\n</root>");
    assert_eq!(format(input, POST), input);
}

/// B-d1f1021402, B-3c550ba920, B-2a3c6b9348, B-c57002bb08
#[test]
fn render_phase_ascii_tables_and_comments() {
    let input = "|`cat <<'EOF' > file`|`write(path=\"file\", content=\"...\")`|\n|`sed -i 's/old/new/' file`|`edit(path=\"file\", edits=[...])`|";
    assert_eq!(
        format(input, PRE_ASCII),
        "|`cat <<'EOF' > file`|`write(path=\"file\", content=\"…\")`|\n|`sed -i 's/old/new/' file`|`edit(path=\"file\", edits=[…])`|"
    );
    let comment = "<!-- Hidden continuation steer. role=user, suppressed from visible transcript. -->";
    assert_eq!(format(comment, PRE_ASCII), comment);
    assert_eq!(format("<!-- -> in comment -->\nvalue -> value", PRE_ASCII), "<!-- -> in comment -->\nvalue → value");
    assert_eq!(format("<!--\nA -> B\n-->", PRE_ASCII), "<!--\nA -> B\n-->");
}

// --- ARA intentional difference -------------------------------------------------

/// Upstream's parser leaves a `{{else if}}` chain open until a second
/// `{{/if}}` ("Parse error: unclosed block if" for the Handlebars form). ARA
/// closes the chain with the single `{{/if}}` Handlebars documents; no
/// upstream prompt uses `else if`.
#[test]
fn else_if_chain_closes_once() {
    let engine = Engine::new();
    let source = "{{#if a}}A{{else if b}}B{{else}}C{{/if}}";
    assert_eq!(engine.render(source, &json!({ "a": true })).unwrap(), "A");
    assert_eq!(engine.render(source, &json!({ "b": true })).unwrap(), "B");
    assert_eq!(engine.render(source, &json!({})).unwrap(), "C");
}
