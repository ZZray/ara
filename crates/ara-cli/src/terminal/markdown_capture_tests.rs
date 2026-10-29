//! Executes the real Rust renderer against all 131 fixed original-test IDs.
//! The fixture contains expected output only. Dependency callback recipes
//! execute the original test theme functions, not a renderer-output lookup.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};

fn chalk(text: &str, recipe: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let codes: &[(u8, u8)] = match recipe {
        "heading" => &[(1, 22), (36, 39)],
        "link" => &[(34, 39)],
        "linkUrl" | "codeBlockBorder" | "quoteBorder" | "hr" => &[(2, 22)],
        "code" | "yellow" => &[(33, 39)],
        "codeBlock" => &[(32, 39)],
        "quote" | "italic" => &[(3, 23)],
        "listBullet" | "cyan" => &[(36, 39)],
        "bold" => &[(1, 22)],
        "strikethrough" => &[(9, 29)],
        "underline" => &[(4, 24)],
        "gray" => &[(90, 39)],
        "magenta" => &[(35, 39)],
        _ => panic!("unknown original callback recipe {recipe}"),
    };
    // Exact fixed pi-utils/chalk.ts applyStyles: closing+opening replacement,
    // reverse close order, and CRLF/LF encasing before outer wrapping.
    let mut output = text.to_owned();
    let (mut opening, mut closing) = (String::new(), String::new());
    for &(on, off) in codes.iter().rev() {
        let a = format!("\x1b[{on}m");
        let b = format!("\x1b[{off}m");
        output = output.replace(&b, &format!("{b}{a}"));
        opening = format!("{a}{opening}");
        closing.push_str(&b);
    }
    let line_re = regex::Regex::new(r"\r?\n").unwrap();
    output = line_re
        .replace_all(&output, |caps: &regex::Captures<'_>| format!("{closing}{}{opening}", &caps[0]))
        .into_owned();
    format!("{opening}{output}{closing}")
}
type Trace = Arc<Mutex<Vec<Value>>>;
fn id_key(id: &Value) -> String {
    id.as_str().map(str::to_owned).unwrap_or_else(|| id.to_string())
}
fn recipe_call(recipe: &Value, args: &[Value]) -> Value {
    let text = args.first().and_then(Value::as_str).unwrap_or("");
    match recipe["kind"].as_str().unwrap() {
        "chalk" => json!(chalk(text, recipe["style"].as_str().unwrap())),
        "link_marker" => json!(format!("<L>{text}</L>")),
        "highlighter_identity" => json!([text]),
        "mermaid_constant" => recipe["output"].clone(),
        "mermaid_exact" => {
            if args[0] == recipe["source"] {
                recipe["output"].clone()
            } else {
                Value::Null
            }
        }
        kind => panic!("unknown original callback kind {kind}"),
    }
}
fn call(trace: &Trace, name: &str, recipe: &Value, args: Vec<Value>) -> Value {
    let result = recipe_call(recipe, &args);
    trace.lock().unwrap().push(json!({"name":name,"args":args,"result":result}));
    result
}
fn make_style(trace: Trace, name: String, recipe: Value) -> StyleFunction {
    Arc::new(move |text| call(&trace, &name, &recipe, vec![json!(text)]).as_str().unwrap().to_owned())
}
/// Exact fixed test-themes.ts defaultMarkdownTheme callbacks/symbols.
/// Reuses independent source recipes only; no expected render output is read.
pub(super) fn fixed_default_markdown_theme() -> (Arc<Theme>, MarkdownOptions) {
    static DEFAULT: OnceLock<(Arc<Theme>, MarkdownOptions)> = OnceLock::new();
    DEFAULT
        .get_or_init(|| {
            let fixture: Value = serde_json::from_str(include_str!("markdown_static_capture.json")).unwrap();
            let trace: Trace = Arc::new(Mutex::new(Vec::new()));
            let native = make_theme(&fixture["themes"]["2"], &fixture["recipes"], &trace);
            let mut options = MarkdownOptions::default();
            options.hyperlinks = false;
            options.style_functions = Some(native.styles);
            options.mermaid = native.mermaid;
            options.highlighter = native.highlighter;
            (native.theme, options)
        })
        .clone()
}

fn flatten_symbols(value: &Value, prefix: &str, out: &mut serde_json::Map<String, Value>) {
    if let Some(object) = value.as_object() {
        for (key, v) in object {
            let p = if prefix.is_empty() { key.clone() } else { format!("{prefix}.{key}") };
            flatten_symbols(v, &p, out);
        }
    } else if value.is_string() {
        out.insert(prefix.to_owned(), value.clone());
    }
}
struct NativeTheme {
    theme: Arc<Theme>,
    styles: MarkdownStyleFunctions,
    mermaid: Option<MermaidResolver>,
    highlighter: Option<CodeHighlighter>,
}
fn make_theme(body: &Value, recipes: &Value, trace: &Trace) -> NativeTheme {
    let mut flat = serde_json::Map::new();
    flatten_symbols(&body["symbols"], "", &mut flat);
    for (src, dst) in [("quoteBorder", "md.quoteBorder"), ("hrChar", "md.hrChar"), ("colorSwatch", "md.colorSwatch")] {
        if let Some(v) = flat.get(src).cloned() {
            flat.insert(dst.to_owned(), v);
        }
    }
    flat.entry("md.colorSwatch").or_insert(json!("■"));
    let theme = Arc::new(Theme::from_json(&json!({"colors":{},"symbols":flat}).to_string(), "ascii").unwrap());
    let mut styles = HashMap::new();
    let mut mermaid = None;
    let mut highlighter = None;
    for (name, id) in body["callbacks"].as_object().unwrap() {
        let recipe = recipes[&id_key(id)].clone();
        let callback_name = format!("theme.{name}");
        let shared = trace.clone();
        match name.as_str() {
            "resolveMermaidAscii" => {
                mermaid = Some(Arc::new(move |source: &str, width: usize| {
                    call(&shared, &callback_name, &recipe, vec![json!(source), json!(width)])
                        .as_str()
                        .map(str::to_owned)
                }) as MermaidResolver);
            }
            "highlightCode" => {
                highlighter = Some(Arc::new(move |source: &str, lang: Option<&str>| {
                    let lang = lang.map(|s| json!(s)).unwrap_or_else(|| json!({"kind":"undefined"}));
                    call(&shared, &callback_name, &recipe, vec![json!(source), lang])
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|v| v.as_str().unwrap().to_owned())
                        .collect()
                }) as CodeHighlighter);
            }
            _ => {
                styles.insert(name.clone(), make_style(shared, callback_name, recipe));
            }
        }
    }
    NativeTheme { theme, styles: Arc::new(styles), mermaid, highlighter }
}
fn options(row: &Value, theme: &NativeTheme, defaults: &HashMap<String, StyleFunction>) -> MarkdownOptions {
    let default = &row["defaultStyle"];
    let get =
        |name: &str| default.get(name).and_then(|v| v.get("id")).and_then(|id| defaults.get(&id_key(id))).cloned();
    MarkdownOptions {
        style_functions: Some(theme.styles.clone()),
        default_style: DefaultTextStyle {
            color: get("color"),
            bg_color: get("bgColor"),
            bold: default["bold"].as_bool().unwrap_or(false),
            italic: default["italic"].as_bool().unwrap_or(false),
            strikethrough: default["strikethrough"].as_bool().unwrap_or(false),
            underline: default["underline"].as_bool().unwrap_or(false),
        },
        padding_x: row["paddingX"].as_u64().unwrap_or(0) as usize,
        padding_y: row["paddingY"].as_u64().unwrap_or(0) as usize,
        code_block_indent: row["codeBlockIndent"].as_u64().unwrap_or(2) as usize,
        hyperlinks: row["terminal"]["hyperlinks"].as_bool().unwrap_or(false),
        text_sizing: row["terminal"]["textSizing"].as_bool().unwrap_or(false),
        true_color: row["terminal"]["trueColor"].as_bool().unwrap_or(false),
        image_protocol: row["terminal"]["imageProtocol"].as_str().map(str::to_owned),
        mermaid: theme.mermaid.clone(),
        highlighter: theme.highlighter.clone(),
        ..Default::default()
    }
}
fn short(value: &str) -> String {
    value.chars().take(220).collect()
}
fn first_difference(actual: &[String], expected: &Value) -> Value {
    let expected = expected.as_array().unwrap();
    for i in 0..actual.len().max(expected.len()) {
        let a = actual.get(i).map(String::as_str);
        let e = expected.get(i).and_then(Value::as_str);
        if a.map(|s| s.encode_utf16().collect::<Vec<_>>()) != e.map(|s| s.encode_utf16().collect::<Vec<_>>()) {
            return json!({"row":i,"actual":a.map(short),"expected":e.map(short),"actualRows":actual.len(),"expectedRows":expected.len()});
        }
    }
    Value::Null
}

#[test]
fn fixed_original_131_static_capture_real_renderer() {
    let fixture: Value = serde_json::from_str(include_str!("markdown_static_capture.json")).unwrap();
    assert_eq!(fixture["fixedCommit"], "596f2da7101178214aa27a753529d15e6b7ad91d");
    assert_eq!(fixture["captureSha256"], "f7f6349bba1b2bea494ec0e685cba93542f26c9d2ee2e60057fadfbb0f5e1897");
    assert_eq!(fixture["originalIds"].as_array().unwrap().len(), 131);
    let trace: Trace = Arc::new(Mutex::new(Vec::new()));
    let recipes = &fixture["recipes"];
    let themes: HashMap<_, _> = fixture["themes"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(id, body)| (id.clone(), make_theme(body, recipes, &trace)))
        .collect();
    let mut defaults = HashMap::new();
    for row in fixture["renders"].as_array().unwrap() {
        if let Some(style) = row["defaultStyle"].as_object() {
            for name in ["color", "bgColor"] {
                if let Some(id) = style.get(name).and_then(|v| v.get("id")) {
                    let key = id_key(id);
                    defaults.entry(key.clone()).or_insert_with(|| {
                        make_style(trace.clone(), format!("defaultStyle.{name}"), recipes[&key].clone())
                    });
                }
            }
        }
    }
    let first: HashMap<u64, &Value> = fixture["renders"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .map(|row| (row["instanceId"].as_u64().unwrap(), row))
        .collect();
    let mut events: Vec<(u64, &str, &Value)> = Vec::new();
    for key in ["actions", "renders", "inline"] {
        for row in fixture[key].as_array().unwrap() {
            events.push((row["sequence"].as_u64().unwrap(), key, row));
        }
    }
    events.sort_by_key(|v| v.0);
    let mut instances: HashMap<u64, (MarkdownComponent, String)> = HashMap::new();
    let mut identities: HashMap<u64, Arc<Vec<String>>> = HashMap::new();
    let mut actual_identities: HashMap<usize, u64> = HashMap::new();
    let mut rows_keep_alive: Vec<Arc<Vec<String>>> = Vec::new();
    let mut results = Vec::new();
    let mut failures = Vec::new();
    let mut visited = BTreeSet::new();
    clear_render_cache();
    super::super::text::set_hangul_compat_jamo_width_override(0);
    for (_, kind, row) in events {
        let instance = row["instanceId"].as_u64().unwrap_or(0);
        if kind == "actions" {
            match row["kind"].as_str().unwrap() {
                "clearRenderCache" => clear_render_cache(),
                "constructor" => {
                    if let Some(first) = first.get(&instance) {
                        let theme = &themes[&id_key(&first["themeIdentity"])];
                        let mut md = MarkdownComponent::new(
                            WireString::from(row["text"].as_str().unwrap()),
                            theme.theme.clone(),
                        );
                        md.set_options(options(first, theme, &defaults));
                        instances.insert(instance, (md, first["terminal"].to_string()));
                    }
                }
                "setText" => {
                    if let Some((md, _)) = instances.get_mut(&instance) {
                        let actual = md.set_text(WireString::from(row["text"].as_str().unwrap()));
                        if actual != row["result"].as_bool().unwrap() {
                            failures.push(json!({"action":"setText","instance":instance,"actual":actual,"expected":row["result"]}));
                        }
                    }
                }
                "setTransientRenderCache" => {
                    let (md, _) = instances.get_mut(&instance).expect("original mode setter after constructor");
                    md.set_transient_render_cache(row["value"].as_bool().unwrap());
                }
                "invalidate" => {
                    if let Some((md, _)) = instances.get_mut(&instance) {
                        md.invalidate(themes[&id_key(&first[&instance]["themeIdentity"])].theme.clone());
                    }
                }
                "setIgnoreTight" => {
                    if let Some((md, _)) = instances.get_mut(&instance) {
                        md.set_ignore_tight(row["ignore"].as_bool().unwrap());
                    }
                }
                other => panic!("unsupported recorded action {other}"),
            }
            continue;
        }
        let id = row["id"].as_str().unwrap();
        visited.insert(id.to_owned());
        trace.lock().unwrap().clear();
        if kind == "inline" {
            let theme = &themes[&id_key(&row["themeIdentity"])];
            let text = WireString::from(row["text"].as_str().unwrap());
            let actual = render_inline_markdown_with_options(
                Some(&text),
                theme.theme.clone(),
                None,
                options(row, theme, &defaults),
            );
            let expected = row["raw"].as_str().unwrap();
            let callbacks = trace.lock().unwrap().clone();
            let ok = actual.encode_utf16().eq(expected.encode_utf16())
                && callbacks == row["callbacks"].as_array().unwrap().as_slice();
            if !ok {
                failures.push(json!({"id":id,"inline":true,"actual":short(&actual),"expected":short(expected),"callbacksMatch":callbacks==row["callbacks"].as_array().unwrap().as_slice()}));
            }
            results.push(json!({"id":id,"kind":"inline","pass":ok,"raw":actual,"callbacks":callbacks}));
            continue;
        }
        assert_eq!(row["hangulWidth"], "platform", "recorded width configuration must be explicitly applied");
        let (md, terminal) = instances.get_mut(&instance).expect("constructor preceding render");
        let key = row["terminal"].to_string();
        if *terminal != key {
            md.set_options(options(row, &themes[&id_key(&row["themeIdentity"])], &defaults));
            *terminal = key;
        }
        let raw = md.render(row["width"].as_u64().unwrap() as usize);
        let callbacks = trace.lock().unwrap().clone();
        let difference = first_difference(&raw, &row["raw"]);
        let expected_id = row["arrayIdentity"].as_u64().unwrap();
        let ptr = Arc::as_ptr(&raw) as usize;
        let same = identities.get(&expected_id).is_none_or(|prior| Arc::ptr_eq(prior, &raw));
        let distinct = actual_identities.get(&ptr).is_none_or(|prior| *prior == expected_id);
        identities.entry(expected_id).or_insert_with(|| raw.clone());
        actual_identities.insert(ptr, expected_id);
        rows_keep_alive.push(raw.clone());
        let callbacks_match = callbacks == row["callbacks"].as_array().unwrap().as_slice();
        let ok = difference.is_null() && callbacks_match && same && distinct;
        if !ok {
            failures.push(json!({"id":id,"instance":instance,"rawDifference":difference,"callbacksMatch":callbacks_match,"actualCallbackCount":callbacks.len(),"expectedCallbackCount":row["callbacks"].as_array().unwrap().len(),"identityMatch":same&&distinct}));
        }
        results.push(json!({"id":id,"kind":"render","pass":ok,"instance":instance,"width":row["width"],"raw":*raw,"callbacks":callbacks,"identityMatch":same&&distinct}));
    }
    clear_render_cache();
    let expected_ids: BTreeSet<String> =
        fixture["originalIds"].as_array().unwrap().iter().map(|id| id.as_str().unwrap().to_owned()).collect();
    assert_eq!(visited, expected_ids, "every retained original ID must actually execute");
    let render_count = results.iter().filter(|r| r["kind"] == "render").count();
    let inline_count = results.iter().filter(|r| r["kind"] == "inline").count();
    assert_eq!(render_count, fixture["renders"].as_array().unwrap().len());
    assert_eq!(inline_count, fixture["inline"].as_array().unwrap().len());
    let receipt = json!({"fixedCommit":fixture["fixedCommit"],"originalIds":visited,"renderCount":render_count,"inlineCount":inline_count,"failedObservations":failures.len(),"failures":failures,"results":results,"boundary":"Real Rust static renderer; not streaming, writer, host, product or complete surface acceptance."});
    if let Ok(path) = std::env::var("ARA_TUI_STATIC_RECEIPT") {
        std::fs::write(path, serde_json::to_vec_pretty(&receipt).unwrap()).unwrap();
    }
    assert!(
        failures.is_empty(),
        "static raw/callback/identity differences: {}",
        serde_json::to_string(&failures).unwrap()
    );
}
