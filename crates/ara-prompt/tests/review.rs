//! Regressions for the independent review of the CTX-01a port. Expected
//! values are upstream's outputs (OMP `template.ts`/`prompt.ts` at 596f2da7)
//! as recorded by the reviewer, except the documented differences.

use ara_prompt::{Engine, Output, render};
use serde_json::json;
use std::time::{Duration, Instant};

/// Run `f` on a thread with the default 2 MiB stack and a timeout.
fn bounded<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(move || {
            let _ = tx.send(f());
        })
        .unwrap();
    rx.recv_timeout(Duration::from_secs(20)).expect("timed out (deadlock or runaway)")
}

/// M1: `each` and path lookups are linear in the context size.
#[test]
fn each_over_a_large_context_is_linear() {
    let items: Vec<_> = (0..20_000).map(|i| json!({ "name": format!("n{i}") })).collect();
    let context = json!({ "items": items, "x": "y" });
    let start = Instant::now();
    let out = Engine::new().render("{{#each items}}- {{name}}\n{{/each}}", &context).unwrap();
    assert_eq!(out.lines().count(), 20_000);
    let lookups = "{{x}}".repeat(500);
    assert_eq!(Engine::new().render(&lookups, &context).unwrap().len(), 500);
    assert!(start.elapsed() < Duration::from_secs(10), "took {:?}", start.elapsed());
}

/// M2: nesting is capped with an error instead of overflowing the stack.
#[test]
fn deep_nesting_fails_cleanly() {
    let (ok, too_deep, subexpr) = bounded(|| {
        let nest = |n: usize| format!("{}x{}", "{{#if a}}".repeat(n), "{{/if}}".repeat(n));
        let engine = Engine::new();
        let ok = engine.render(&nest(ara_prompt::template::MAX_NESTING), &json!({ "a": true }));
        let too_deep = engine.render(&nest(2000), &json!({ "a": true }));
        let deep_subexpr = format!("{{{{lookup {}a{} 0}}}}", "(lookup ".repeat(5000), " 0)".repeat(5000));
        (ok, too_deep, engine.render(&deep_subexpr, &json!({})))
    });
    assert_eq!(ok.unwrap(), "x");
    assert_eq!(too_deep.unwrap_err().0, "Template nesting exceeds 100 levels");
    assert_eq!(subexpr.unwrap_err().0, "Template nesting exceeds 100 levels");
}

/// M3: helpers may render and register re-entrantly on the shared engine.
#[test]
fn helpers_reenter_the_shared_engine() {
    let out = bounded(|| {
        ara_prompt::register_helper("review_nested", |h| {
            ara_prompt::register_partial("review_unused", "u");
            let inner = ara_prompt::render("inner {{n}}", &json!({ "n": h.arg(0) }))?;
            Ok(Output::from(inner))
        });
        let writer = std::thread::spawn(|| {
            for i in 0..200 {
                ara_prompt::register_partial(&format!("review_spin{i}"), "s");
            }
        });
        let mut last = String::new();
        for _ in 0..200 {
            last = render("outer {{review_nested 1}}", &json!({})).unwrap();
        }
        writer.join().unwrap();
        last
    });
    assert_eq!(out, "outer inner 1");
}

/// L1: integer-like keys enumerate first, ascending (JS own-key order).
#[test]
fn integer_like_keys_enumerate_first() {
    let out =
        render("{{#each m}}{{@key}}={{this}};{{/each}}|{{jsonStringify m}}", &json!({ "m": { "b": 1, "10": 2 } }));
    assert_eq!(out.unwrap(), r#"10=2;b=1;|{"10":2,"b":1}"#);
}

/// L2, L3: non-finite arithmetic and JS number formatting.
#[test]
fn non_finite_and_exponent_numbers() {
    let out =
        render("{{sub s 1}}|{{add big big}}|{{a}}|{{b}}", &json!({ "s": "abc", "big": 1e308, "a": 1e21, "b": 1e-7 }));
    assert_eq!(out.unwrap(), "NaN|Infinity|1e+21|1e-7");
}

/// L4: standalone detection uses JavaScript's `\s` and ASCII `\b`.
#[test]
fn standalone_lines_use_js_whitespace() {
    let engine = Engine::new();
    let ctx = json!({ "x": true });
    assert_eq!(engine.render("\u{FEFF}{{#if x}}\nA\n{{/if}}\nB", &ctx).unwrap(), "A\nB");
    assert_eq!(engine.render("\u{85}{{#if x}}\nA\n{{/if}}\nB", &ctx).unwrap(), "\u{85}\nA\nB");
    assert_eq!(engine.render("{{elseé}}\nx", &ctx).unwrap(), "x");
}

/// L5: partials see only the built-ins, like upstream's string partials.
#[test]
fn partials_render_with_builtins_only() {
    ara_prompt::register_partial("review_p", "[{{join xs}}]");
    ara_prompt::register_partial("review_q", "<{{> review_p}}>");
    let ctx = json!({ "xs": [1, 2] });
    assert_eq!(render("{{> review_p}}", &ctx).unwrap_err().0, "Missing helper: \"join\"");
    assert_eq!(render("{{> review_q}}", &ctx).unwrap_err().0, "The partial review_p could not be found");
    let mut engine = Engine::new();
    engine.register_partial("row", "{{#each xs}}{{this}};{{/each}}{{k}}");
    assert_eq!(engine.render("{{> row k=\"!\"}}", &ctx).unwrap(), "1;2;!");
    assert_eq!(engine.render("{{> row items}}", &json!({ "items": ["a"] })).unwrap(), "");
}

/// L6 and error paths: double-closed else-if and non-string separators fail.
#[test]
fn documented_error_paths() {
    let engine = Engine::new();
    assert_eq!(
        engine.render("{{#if a}}A{{else if b}}B{{/if}}{{/if}}", &json!({ "b": true })).unwrap_err().0,
        "Parse error: mismatched /if"
    );
    assert!(render("{{#list xs join=5}}{{this}}{{/list}}", &json!({ "xs": [1] })).is_err());
    assert!(render("{{#table xs headers=5}}{{this}}{{/table}}", &json!({ "xs": [1] })).is_err());
}
