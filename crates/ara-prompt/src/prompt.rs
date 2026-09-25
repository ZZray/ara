//! Prompt rendering seam (OMP `packages/utils/src/prompt.ts` at
//! 596f2da7101178214aa27a753529d15e6b7ad91d): the shared helper set, the
//! compile cache, `render` (no HTML escaping, then post-render `format`) and
//! the `format` pipeline that normalizes mixed XML/Markdown/Handlebars text.

use crate::js;
use crate::template::{CompileOptions, Engine, HelperCall, Output, Template, TemplateError, property};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, RwLock};

// ---------------------------------------------------------------------------
// format
// ---------------------------------------------------------------------------

/// Which side of template rendering `format` runs on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RenderPhase {
    /// Source formatting: blank lines before `{{/…}}` closers are dropped too.
    PreRender,
    #[default]
    PostRender,
}

/// `PromptFormatOptions`.
#[derive(Clone, Copy, Debug, Default)]
pub struct FormatOptions {
    pub render_phase: RenderPhase,
    pub replace_ascii_symbols: bool,
    pub normalize_rfc2119: bool,
}

impl FormatOptions {
    /// The options OMP's `format-prompts` script applies to prompt sources.
    pub const PROMPT_SOURCE: FormatOptions =
        FormatOptions { render_phase: RenderPhase::PreRender, replace_ascii_symbols: true, normalize_rfc2119: true };
}

fn is_tag_char(b: u8) -> bool {
    b.is_ascii_lowercase() || b == b'-' || b == b'_'
}

/// `^</([a-z_-]+)>$` (caller guarantees a `</` prefix).
fn closing_tag_name(s: &str) -> Option<&str> {
    let b = s.as_bytes();
    let n = b.len();
    if n < 4 || b[n - 1] != b'>' {
        return None;
    }
    b[2..n - 1].iter().all(|&c| is_tag_char(c)).then(|| &s[2..n - 1])
}

/// `^<([a-z_-]+)(?:\s+[^>]*)?>$` (caller guarantees a `<` prefix, not `</`).
fn opening_tag_name(s: &str) -> Option<&str> {
    let b = s.as_bytes();
    let n = b.len();
    if n < 3 || b[n - 1] != b'>' {
        return None;
    }
    let mut j = 1;
    while j < n - 1 && is_tag_char(b[j]) {
        j += 1;
    }
    if j == 1 {
        return None;
    }
    if j == n - 1 {
        return Some(&s[1..j]);
    }
    // ASCII: only space and tab; non-ASCII: the regex's `\s`.
    let next = s[j..].chars().next()?;
    if !(next == ' ' || next == '\t' || (!next.is_ascii() && js::is_space(next))) {
        return None;
    }
    (!s[j..n - 1].contains('>')).then(|| &s[1..j])
}

/// `^\|[-:\s|]+\|$`
fn is_table_sep(s: &str) -> bool {
    let Some(inner) = s.strip_prefix('|').and_then(|r| r.strip_suffix('|')) else { return false };
    !inner.is_empty() && inner.chars().all(|c| matches!(c, '-' | ':' | '|') || js::is_space(c))
}

/// `^\|.*\|$` (`.` excludes line terminators).
fn is_table_row(s: &str) -> bool {
    let Some(inner) = s.strip_prefix('|').and_then(|r| r.strip_suffix('|')) else { return false };
    !inner.contains(['\n', '\r', '\u{2028}', '\u{2029}'])
}

fn compact_table_row(line: &str) -> String {
    line.split('|').map(js::trim).collect::<Vec<_>>().join("|")
}

fn compact_table_sep(line: &str) -> String {
    let cells: Vec<&str> = line
        .split('|')
        .map(js::trim)
        .filter(|c| !c.is_empty())
        .map(|c| match (c.starts_with(':'), c.ends_with(':')) {
            (true, true) => ":---:",
            (true, false) => ":---",
            (false, true) => "---:",
            (false, false) => "---",
        })
        .collect();
    format!("|{}|", cells.join("|"))
}

static RFC2119_BOLD: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\*\*(MUST NOT|SHOULD NOT|RECOMMENDED|REQUIRED|OPTIONAL|SHOULD|MUST|MAY|NEVER|AVOID)\*\*")
        .unwrap()
});
static RFC2119_GUARD: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\*\*(?:MUST|SHOULD|RECOMMENDED|REQUIRED|OPTIONAL|MAY|NEVER|AVOID)|MUST NOT|SHOULD NOT").unwrap()
});
static ASCII_SYMBOLS: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\.{3}|<->|->|<-|!=|<=|>=").unwrap());

/// Replace `\bNEEDLE\b` (JS ASCII word boundaries) with `with`.
fn replace_word(text: &str, needle: &str, with: &str) -> String {
    let is_word = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    while let Some(rel) = text[cursor..].find(needle) {
        let start = cursor + rel;
        let end = start + needle.len();
        if !is_word(text[..start].chars().next_back()) && !is_word(text[end..].chars().next()) {
            out.push_str(&text[cursor..start]);
            out.push_str(with);
            cursor = end;
        } else {
            let step = text[start..].chars().next().map_or(1, char::len_utf8);
            out.push_str(&text[cursor..start + step]);
            cursor = start + step;
        }
    }
    out.push_str(&text[cursor..]);
    out
}

fn apply_rfc2119(text: &str) -> String {
    let unbolded = RFC2119_BOLD.replace_all(text, "$1");
    replace_word(&replace_word(&unbolded, "MUST NOT", "NEVER"), "SHOULD NOT", "AVOID")
}

/// Strip `**KEYWORD**` bold and alias `MUST NOT`/`SHOULD NOT` outside inline code.
fn normalize_rfc2119(line: &str) -> String {
    if !RFC2119_GUARD.is_match(line) {
        return line.to_string();
    }
    if !line.contains('`') {
        return apply_rfc2119(line);
    }
    line.split('`')
        .enumerate()
        .map(|(i, segment)| if i % 2 == 0 { apply_rfc2119(segment) } else { segment.to_string() })
        .collect::<Vec<_>>()
        .join("`")
}

fn replace_ascii(text: &str) -> String {
    ASCII_SYMBOLS
        .replace_all(text, |caps: &regex::Captures<'_>| match &caps[0] {
            "..." => "…",
            "<->" => "↔",
            "->" => "→",
            "<-" => "←",
            "!=" => "≠",
            "<=" => "≤",
            _ => "≥",
        })
        .into_owned()
}

const COMMENT_OPEN: &str = "<!--";
const COMMENT_CLOSE: &str = "-->";

fn replace_ascii_outside_comments(line: &str, in_comment: &mut bool) -> String {
    if !*in_comment && !line.contains(COMMENT_OPEN) {
        return replace_ascii(line);
    }
    let mut out = String::new();
    let mut cursor = 0;
    while cursor < line.len() {
        if *in_comment {
            let Some(close) = line[cursor..].find(COMMENT_CLOSE).map(|i| cursor + i) else {
                out.push_str(&line[cursor..]);
                return out;
            };
            out.push_str(&line[cursor..close + COMMENT_CLOSE.len()]);
            cursor = close + COMMENT_CLOSE.len();
            *in_comment = false;
            continue;
        }
        let Some(open) = line[cursor..].find(COMMENT_OPEN).map(|i| cursor + i) else {
            out.push_str(&replace_ascii(&line[cursor..]));
            return out;
        };
        out.push_str(&replace_ascii(&line[cursor..open]));
        let search = open + COMMENT_OPEN.len();
        let Some(close) = line[search..].find(COMMENT_CLOSE).map(|i| search + i) else {
            out.push_str(&line[open..]);
            *in_comment = true;
            return out;
        };
        out.push_str(&line[open..close + COMMENT_CLOSE.len()]);
        cursor = close + COMMENT_CLOSE.len();
    }
    out
}

/// Indent width in bytes and the first non-indent char. Spaces and tabs are
/// the indent; a non-ASCII first char defers to the full JS `trimStart`.
fn indent(line: &str) -> (usize, Option<char>) {
    let s = line.bytes().position(|b| b != b' ' && b != b'\t').unwrap_or(line.len());
    match line[s..].chars().next() {
        Some(c) if !c.is_ascii() => {
            let s = line.len() - js::trim_start(line).len();
            (s, line[s..].chars().next())
        }
        first => (s, first),
    }
}

fn is_blank(line: &str) -> bool {
    js::trim(line).is_empty()
}

fn pop_blanks(result: &mut Vec<String>) {
    while result.last().is_some_and(String::is_empty) {
        result.pop();
    }
}

/// Normalize prompt text (upstream `format`): trim line ends, keep code fences
/// verbatim, optionally replace ASCII symbols outside HTML comments, compact
/// tables, optionally normalize RFC 2119 keywords, collapse blank runs and
/// drop blanks before closing XML tags (and `{{/…}}` in pre-render).
pub fn format(content: &str, options: FormatOptions) -> String {
    let pre_render = options.render_phase == RenderPhase::PreRender;
    let lines: Vec<&str> = content.split('\n').collect();
    let mut result: Vec<String> = Vec::with_capacity(lines.len());
    let mut in_code_block = false;
    let mut in_comment = false;
    let mut top_level_tags: Vec<String> = Vec::new();

    let mut i = 0;
    while i < lines.len() {
        let mut line = js::trim_end(lines[i]).to_string();
        let (mut s, mut first) = indent(&line);

        if matches!(first, Some('`' | '~')) && (line[s..].starts_with("```") || line[s..].starts_with("~~~")) {
            in_code_block = !in_code_block;
            result.push(line);
            i += 1;
            continue;
        }
        if in_code_block {
            result.push(line);
            i += 1;
            continue;
        }

        if options.replace_ascii_symbols {
            let replaced = replace_ascii_outside_comments(&line, &mut in_comment);
            if replaced != line {
                line = replaced;
                (s, first) = indent(&line);
            }
        }

        let mut is_closing_line = false;
        if first == Some('<') {
            let trimmed = &line[s..];
            if trimmed.as_bytes().get(1) == Some(&b'/') {
                if let Some(name) = closing_tag_name(trimmed) {
                    is_closing_line = true;
                    if top_level_tags.last().is_some_and(|top| top == name) {
                        top_level_tags.pop();
                    }
                }
            } else if s == 0
                && !trimmed.ends_with("/>")
                && let Some(name) = opening_tag_name(trimmed)
            {
                top_level_tags.push(name.to_string());
            }
        } else if first == Some('|') {
            let trimmed = &line[s..];
            if is_table_sep(trimmed) {
                line = format!("{}{}", &line[..s], compact_table_sep(trimmed));
            } else if is_table_row(trimmed) {
                line = format!("{}{}", &line[..s], compact_table_row(trimmed));
            }
        }

        if options.normalize_rfc2119 {
            line = normalize_rfc2119(&line);
        }

        if s >= line.len() {
            // Blank line: a run of 2+ blanks (or a trailing blank) disappears
            // entirely; a single blank survives unless it would lead.
            if lines.get(i + 1).is_none_or(|next| is_blank(next)) {
                pop_blanks(&mut result);
                let mut j = i + 1;
                while j < lines.len() && is_blank(lines[j]) {
                    j += 1;
                }
                i = j;
                continue;
            }
            if result.last().is_none_or(String::is_empty) {
                i += 1;
                continue;
            }
        }

        if is_closing_line || (pre_render && first == Some('{') && line[s..].starts_with("{{/")) {
            pop_blanks(&mut result);
        }
        result.push(line);
        i += 1;
    }
    pop_blanks(&mut result);
    result.join("\n")
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

type HelperResult = std::result::Result<Output, TemplateError>;

fn text(s: impl Into<String>) -> HelperResult {
    Ok(Output::Value(Value::String(s.into())))
}

/// `value ?? fallback` rendered through `String()`.
fn string_or(value: Option<&Value>, fallback: &str) -> String {
    match value {
        None | Some(Value::Null) => fallback.to_string(),
        Some(v) => js::to_string(v),
    }
}

fn unescape_separator(separator: &str) -> String {
    separator.replace("\\n", "\n").replace("\\t", "\t")
}

/// `Array.prototype.join`: `null` entries join as empty strings.
fn join_items(items: &[Value], separator: &str) -> String {
    items
        .iter()
        .map(|item| if item.is_null() { String::new() } else { js::to_string(item) })
        .collect::<Vec<_>>()
        .join(separator)
}

/// Keys every plain object answers to through `in` (`Object.prototype`).
const OBJECT_PROTOTYPE_KEYS: &[&str] = &[
    "__defineGetter__",
    "__defineSetter__",
    "__lookupGetter__",
    "__lookupSetter__",
    "__proto__",
    "constructor",
    "hasOwnProperty",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toLocaleString",
    "toString",
    "valueOf",
];

/// Array `includes` (SameValueZero; distinct objects never match).
fn includes(items: &[Value], item: &Value) -> bool {
    items.iter().any(|candidate| js::strict_eq(candidate, item))
}

fn helper_arg(h: &HelperCall<'_>) -> HelperResult {
    let index = h.arg(0);
    let parsed = match index {
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        other => js::parse_int(other),
    };
    if !parsed.is_finite() || parsed - 1.0 < 0.0 {
        return text("");
    }
    let key = js::to_string(&js::number(parsed - 1.0));
    let value = match property(h.this(), "args") {
        Value::String(s) => key
            .parse::<usize>()
            .ok()
            .and_then(|i| s.encode_utf16().nth(i))
            .map_or(Value::Null, |unit| Value::String(String::from_utf16_lossy(&[unit]))),
        args => property(&args, &key),
    };
    Ok(Output::Value(if value.is_null() { Value::String(String::new()) } else { value }))
}

fn helper_list(h: &HelperCall<'_>) -> HelperResult {
    let Value::Array(items) = h.arg(0) else { return text("") };
    let prefix = string_or(h.hash.get("prefix"), "");
    let suffix = string_or(h.hash.get("suffix"), "");
    // Upstream calls `.replace` on the value, which throws for a non-string.
    let separator = match h.hash.get("join") {
        None | Some(Value::Null) => "\n".to_string(),
        Some(Value::String(s)) => unescape_separator(s),
        Some(_) => return Err(TemplateError("list: `join` must be a string".into())),
    };
    let rendered = items
        .iter()
        .map(|item| Ok(format!("{prefix}{}{suffix}", h.render(item)?)))
        .collect::<std::result::Result<Vec<_>, TemplateError>>()?;
    text(rendered.join(&separator))
}

fn helper_join(h: &HelperCall<'_>) -> HelperResult {
    let Value::Array(items) = h.arg(0) else { return text("") };
    let separator = match h.arg(1) {
        Value::String(s) => unescape_separator(s),
        _ => ", ".to_string(),
    };
    text(join_items(items, &separator))
}

fn helper_when(h: &HelperCall<'_>) -> HelperResult {
    use std::cmp::Ordering::{Equal, Greater, Less};
    let (lhs, rhs) = (h.arg(0), h.arg(2));
    let result = match js::to_string(h.arg(1)).as_str() {
        "==" | "===" => js::strict_eq(lhs, rhs),
        "!=" | "!==" => !js::strict_eq(lhs, rhs),
        ">" => js::compare(lhs, rhs) == Some(Greater),
        "<" => js::compare(lhs, rhs) == Some(Less),
        ">=" => matches!(js::compare(lhs, rhs), Some(Greater | Equal)),
        "<=" => matches!(js::compare(lhs, rhs), Some(Less | Equal)),
        _ => return text(h.inverse(h.this())?),
    };
    text(if result { h.render(h.this())? } else { h.inverse(h.this())? })
}

fn helper_table(h: &HelperCall<'_>) -> HelperResult {
    let Value::Array(items) = h.arg(0) else { return text("") };
    if items.is_empty() {
        return text("");
    }
    let headers: Vec<String> = match h.hash.get("headers") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::String(s)) => s.split('|').map(str::to_string).collect(),
        // Upstream calls `.split` on the value, which throws for a non-string.
        Some(_) => return Err(TemplateError("table: `headers` must be a string".into())),
    };
    let header_row = if headers.is_empty() {
        String::new()
    } else {
        let separator = vec!["---"; headers.len()].join(" | ");
        format!("| {} |\n| {separator} |\n", headers.join(" | "))
    };
    let rows = items
        .iter()
        .map(|item| Ok(format!("| {} |", js::trim(&h.render(item)?))))
        .collect::<std::result::Result<Vec<_>, TemplateError>>()?;
    text(header_row + &rows.join("\n"))
}

fn helper_has(h: &HelperCall<'_>) -> HelperResult {
    let item = h.arg(1);
    let found = match h.arg(0) {
        Value::Array(items) => includes(items, item),
        Value::Object(map) => match item {
            Value::String(_) | Value::Number(_) => {
                let key = js::to_string(item);
                map.contains_key(&key) || OBJECT_PROTOTYPE_KEYS.contains(&key.as_str())
            }
            _ => false,
        },
        _ => false,
    };
    text(if found { h.render(h.this())? } else { h.inverse(h.this())? })
}

fn builtin_engine() -> Engine {
    let mut engine = Engine::with_options(CompileOptions { no_escape: true });
    engine.register_helper("arg", helper_arg);
    engine.register_helper("list", helper_list);
    engine.register_helper("join", helper_join);
    engine.register_helper("default", |h| {
        Ok(Output::Value(if js::truthy(h.arg(0)) { h.arg(0) } else { h.arg(1) }.clone()))
    });
    engine.register_helper("pluralize", |h| {
        let count = h.arg(0);
        let word = if js::strict_eq(count, &Value::from(1)) { h.arg(1) } else { h.arg(2) };
        text(format!("{} {}", js::to_string(count), js::to_string(word)))
    });
    engine.register_helper("when", helper_when);
    engine.register_helper("ifAny", |h| {
        text(if h.args.iter().any(js::truthy) { h.render(h.this())? } else { h.inverse(h.this())? })
    });
    engine.register_helper("ifAll", |h| {
        text(if h.args.iter().all(js::truthy) { h.render(h.this())? } else { h.inverse(h.this())? })
    });
    engine.register_helper("table", helper_table);
    engine.register_helper("codeblock", |h| {
        let lang = string_or(h.hash.get("lang"), "");
        let content = h.render(h.this())?;
        text(format!("```{lang}\n{}\n```", js::trim(&content)))
    });
    engine.register_helper("xml", |h| {
        let content = h.render(h.this())?;
        let content = js::trim(&content);
        if content.is_empty() {
            return text("");
        }
        let tag = js::to_string(h.arg(0));
        text(format!("<{tag}>\n{content}\n</{tag}>"))
    });
    engine.register_helper("escapeXml", |h| match h.arg(0) {
        Value::Null => text(""),
        v => text(
            js::to_string(v).replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;"),
        ),
    });
    engine.register_helper("len", |h| {
        Ok(Output::Value(js::number(match h.arg(0) {
            Value::Array(items) => items.len() as f64,
            Value::String(s) => js::utf16_len(s) as f64,
            _ => 0.0,
        })))
    });
    let or_zero = |v: &Value| if v.is_null() { Value::from(0) } else { v.clone() };
    engine.register_helper("add", move |h| Ok(Output::Value(js::add(&or_zero(h.arg(0)), &or_zero(h.arg(1))))));
    engine.register_helper("sub", move |h| {
        Ok(Output::Value(js::number(js::to_number(&or_zero(h.arg(0))) - js::to_number(&or_zero(h.arg(1))))))
    });
    engine.register_helper("has", helper_has);
    engine.register_helper("includes", |h| {
        Ok(Output::from(matches!(h.arg(0), Value::Array(items) if includes(items, h.arg(1)))))
    });
    engine.register_helper("not", |h| Ok(Output::from(!js::truthy(h.arg(0)))));
    // `JSON.stringify(undefined)` is `undefined`, which renders empty.
    engine.register_helper("jsonStringify", |h| match h.arg(0) {
        Value::Null => Ok(Output::Value(Value::Null)),
        v => text(js::json_stringify(v)),
    });
    engine
}

/// The shared engine. Renders take an `Arc` snapshot and hold no lock, so a
/// helper may render or register re-entrantly; registering clones the
/// engine when a render still holds the old snapshot.
static ENGINE: LazyLock<RwLock<Arc<Engine>>> = LazyLock::new(|| RwLock::new(Arc::new(builtin_engine())));
static CACHE: LazyLock<Mutex<HashMap<String, Arc<Template>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn engine() -> Arc<Engine> {
    Arc::clone(&ENGINE.read().unwrap_or_else(std::sync::PoisonError::into_inner))
}

fn update_engine(f: impl FnOnce(&mut Engine)) {
    let mut guard = ENGINE.write().unwrap_or_else(std::sync::PoisonError::into_inner);
    f(Arc::make_mut(&mut guard));
}

/// Register a helper on the shared prompt engine.
pub fn register_helper(
    name: &str,
    helper: impl Fn(&HelperCall<'_>) -> std::result::Result<Output, TemplateError> + Send + Sync + 'static,
) {
    update_engine(|engine| engine.register_helper(name, helper));
}

/// Register a partial on the shared prompt engine.
pub fn register_partial(name: &str, source: &str) {
    update_engine(|engine| engine.register_partial(name, source));
}

/// Compile through the shared engine; repeat compiles of the same source
/// return the same cached template.
pub fn compile(source: &str) -> std::result::Result<Arc<Template>, TemplateError> {
    if let Some(template) = CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(source) {
        return Ok(Arc::clone(template));
    }
    let template = Arc::new(engine().compile(source)?);
    let mut cache = CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    Ok(Arc::clone(cache.entry(source.to_string()).or_insert(template)))
}

/// Render a prompt template (no HTML escaping) and post-render `format` it.
pub fn render(source: &str, context: &Value) -> std::result::Result<String, TemplateError> {
    let template = compile(source)?;
    let rendered = engine().render_template(&template, context)?;
    Ok(format(&rendered, FormatOptions::default()))
}
