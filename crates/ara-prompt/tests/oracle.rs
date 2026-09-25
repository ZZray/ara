//! Differential tests against OMP's own `template.ts`/`prompt.ts`, executed
//! by `scripts/prompt_oracle.ts` at the pinned upstream commit.

use ara_prompt::{Engine, FormatOptions, Output, RenderPhase, TemplateError};
use serde_json::Value;
use std::path::Path;

/// The escaping engine with the helpers from upstream `template.test.ts`.
fn test_engine() -> Engine {
    let mut engine = Engine::new();
    engine.register_helper("eq", |h| {
        Ok(Output::from(match (h.arg(0), h.arg(1)) {
            (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
            (Value::Array(_) | Value::Object(_), _) => false,
            (a, b) => a == b,
        }))
    });
    engine.register_helper("label", |h| {
        let prefix = h.hash.get("prefix").map_or("undefined".to_string(), js_string);
        Ok(Output::from(format!("{prefix}:{}", js_string(h.arg(0)))))
    });
    engine.register_helper("choose", |h| {
        let expected = h.hash.get("expected").unwrap_or(&Value::Null);
        let same = match (h.arg(0), expected) {
            (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
            (a, b) => a == b && !a.is_array() && !a.is_object(),
        };
        Ok(Output::from(if same { h.render(h.this())? } else { h.inverse(h.this())? }))
    });
    engine.register_helper("safe", |h| Ok(Output::Safe(js_string(h.arg(0)))));
    engine
}

fn js_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "undefined".into(),
        other => other.to_string(),
    }
}

fn format_options(v: &Value) -> FormatOptions {
    FormatOptions {
        render_phase: if v["renderPhase"] == "pre-render" { RenderPhase::PreRender } else { RenderPhase::PostRender },
        replace_ascii_symbols: v["replaceAsciiSymbols"] == true,
        normalize_rfc2119: v["normalizeRfc2119"] == true,
    }
}

fn outcome(result: Result<String, TemplateError>) -> Value {
    match result {
        Ok(text) => serde_json::json!({ "ok": text }),
        Err(error) => serde_json::json!({ "error": error.0 }),
    }
}

fn expected(case: &Value) -> Value {
    match case.get("ok") {
        Some(ok) => serde_json::json!({ "ok": ok }),
        None => serde_json::json!({ "error": case["error"] }),
    }
}

#[test]
fn generated_cases_match_upstream() {
    let raw =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/oracle.json")).unwrap();
    let oracle: Value = serde_json::from_str(&raw).unwrap();
    let contexts = oracle["contexts"].as_array().unwrap();
    let cases = oracle["cases"].as_array().unwrap();
    let engine = test_engine();
    let mut failures = Vec::new();
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    for case in cases {
        let source = case["source"].as_str().unwrap();
        let op = case["op"].as_str().unwrap();
        let actual = match op {
            "render" => outcome(ara_prompt::render(source, &contexts[case["context"].as_u64().unwrap() as usize])),
            "engine" => outcome(engine.render(source, &contexts[case["context"].as_u64().unwrap() as usize])),
            "format" => outcome(Ok(ara_prompt::format(source, format_options(&case["options"])))),
            other => panic!("unknown op {other}"),
        };
        *counts.entry(op.to_string()).or_default() += 1;
        if actual != expected(case) {
            failures.push(format!(
                "{} ({op}) source={source:?}\n  upstream={}\n  ara     ={}",
                case["id"],
                expected(case),
                actual
            ));
        }
    }
    assert!(cases.len() >= 700, "oracle has {} cases", cases.len());
    println!("oracle cases by op: {counts:?}");
    assert!(failures.is_empty(), "{} of {} cases differ:\n{}", failures.len(), cases.len(), failures.join("\n"));
}

/// Every upstream `packages/*/src/**/*.md`: compile outcome, prompt-source
/// format and post-render format equal upstream's. Needs
/// `ARA_PROMPT_CORPUS_ORACLE` (from `prompt_oracle.ts corpus`) and
/// `ARA_OMP_ROOT`; skips itself otherwise.
#[test]
fn upstream_prompt_corpus_matches() {
    let (Ok(oracle_path), Ok(root)) = (std::env::var("ARA_PROMPT_CORPUS_ORACLE"), std::env::var("ARA_OMP_ROOT")) else {
        println!("skipped: ARA_PROMPT_CORPUS_ORACLE / ARA_OMP_ROOT not set");
        return;
    };
    let oracle: Value = serde_json::from_str(&std::fs::read_to_string(oracle_path).unwrap()).unwrap();
    let files = oracle["files"].as_array().unwrap();
    let mut failures = Vec::new();
    let mut compiled = 0;
    for file in files {
        let rel = file["path"].as_str().unwrap();
        let source = std::fs::read_to_string(Path::new(&root).join(rel)).unwrap();
        let compile = match ara_prompt::compile(&source) {
            Ok(_) => serde_json::json!({ "ok": "ok" }),
            Err(error) => serde_json::json!({ "error": error.0 }),
        };
        if compile.get("ok").is_some() && source.contains("{{") {
            compiled += 1;
        }
        let checks = [
            ("compile", compile, expected(&file["compile"])),
            (
                "source_format",
                outcome(Ok(ara_prompt::format(&source, FormatOptions::PROMPT_SOURCE))),
                expected(&file["source_format"]),
            ),
            (
                "post_format",
                outcome(Ok(ara_prompt::format(&source, FormatOptions::default()))),
                expected(&file["post_format"]),
            ),
        ];
        for (what, actual, want) in checks {
            if actual != want {
                failures.push(format!("{rel} {what}"));
            }
        }
    }
    println!("corpus: {} files, {compiled} templates compiled", files.len());
    assert!(files.len() > 200 && compiled > 100);
    assert!(failures.is_empty(), "{} corpus mismatches:\n{}", failures.len(), failures.join("\n"));
}
