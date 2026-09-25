//! Markdown YAML frontmatter (OMP `packages/utils/src/frontmatter.ts`).
//!
//! YAML is read from `yaml-rust2`'s event stream (iteratively, nesting capped
//! at [`MAX_YAML_DEPTH`]) and resolved with the YAML 1.2 core schema:
//! `null`/`Null`/`NULL`/`~`/empty are null, `true`/`True`/`TRUE`/`false`/…
//! booleans, decimal/`0o`/`0x` integers, floats; `yes`, dates and `1_000`
//! stay strings. Anchors, aliases and `<<` merge keys resolve; a repeated key
//! keeps the last value; integer-like keys come first (JS object order);
//! several documents parse as an array (so no mapping).
//!
//! Known differences from `Bun.YAML` (upstream's parser), from a
//! side-by-side review: `.nan` reads as null and `±.inf` as `±f64::MAX`
//! (JSON has no non-finite numbers; both keep JS truthiness and non-string
//! type); Bun's number quirks (`+.5` and `-.5` as strings, signed hex, `1e`)
//! follow the spec here instead; block-scalar edge cases (`|+` at the end of
//! frontmatter, explicit indentation indicators), `description: ---` as a
//! document separator, `a: ? b`, tabs outside `repair`, complex keys and
//! self-referencing aliases may differ. Deep nesting fails with an error
//! (and the line fallback) instead of recursing.

use serde_json::{Map, Value};
use std::sync::LazyLock;
use yaml_rust2::parser::{Event, MarkedEventReceiver, Parser, Tag};

/// Maximum YAML nesting depth; deeper input is a parse error.
pub const MAX_YAML_DEPTH: usize = 256;
use yaml_rust2::scanner::{Marker, TScalarStyle};

/// How a parse failure is reported (`level`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FailureLevel {
    Off,
    /// Return the fallback parse with a warning (default).
    #[default]
    Warn,
    /// Fail with the error.
    Fatal,
}

/// `FrontmatterOptions`.
#[derive(Clone, Debug)]
pub struct FrontmatterOptions {
    /// Where the content came from, for warnings.
    pub source: Option<String>,
    pub fallback: Map<String, Value>,
    /// Normalize CRLF/CR to LF.
    pub normalize: bool,
    pub level: FailureLevel,
    /// Lenient recovery: strip HTML comments, tabs to spaces, quote
    /// ambiguous plain scalars.
    pub repair: bool,
    /// Keep keys verbatim instead of kebab-case → camelCase.
    pub raw_keys: bool,
}

impl Default for FrontmatterOptions {
    fn default() -> Self {
        FrontmatterOptions {
            source: None,
            fallback: Map::new(),
            normalize: true,
            level: FailureLevel::Warn,
            repair: true,
            raw_keys: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Frontmatter {
    pub frontmatter: Map<String, Value>,
    pub body: String,
    /// `Failed to parse YAML frontmatter (<source>): <error>` when the YAML
    /// was unrecoverable and the line fallback was used.
    pub warning: Option<String>,
}

// ---------------------------------------------------------------------------
// YAML → JSON (core schema)
// ---------------------------------------------------------------------------

static CORE_INT: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"^[-+]?[0-9]+$").unwrap());
static CORE_FLOAT: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[-+]?(?:\.[0-9]+|[0-9]+(?:\.[0-9]*)?)(?:[eE][-+]?[0-9]+)?$").unwrap());

fn number(f: f64) -> Value {
    finite(f)
}

/// JSON has no non-finite numbers: NaN (falsy) becomes null and ±Infinity
/// (truthy numbers) `±f64::MAX`, keeping upstream's truthiness and type.
fn finite(f: f64) -> Value {
    if f.is_nan() {
        Value::Null
    } else if f.is_infinite() {
        ara_prompt::js::number(if f > 0.0 { f64::MAX } else { f64::MIN })
    } else {
        ara_prompt::js::number(f)
    }
}

/// Core-schema resolution of a plain scalar.
fn resolve_plain(v: &str) -> Value {
    match v {
        "" | "~" | "null" | "Null" | "NULL" => return Value::Null,
        "true" | "True" | "TRUE" => return Value::Bool(true),
        "false" | "False" | "FALSE" => return Value::Bool(false),
        ".inf" | ".Inf" | ".INF" | "+.inf" | "+.Inf" | "+.INF" => return finite(f64::INFINITY),
        "-.inf" | "-.Inf" | "-.INF" => return finite(f64::NEG_INFINITY),
        ".nan" | ".NaN" | ".NAN" => return finite(f64::NAN),
        _ => {}
    }
    if let Some(hex) = v.strip_prefix("0x").filter(|h| !h.is_empty() && h.chars().all(|c| c.is_ascii_hexdigit())) {
        return number(hex.chars().fold(0.0, |acc, c| acc * 16.0 + f64::from(c.to_digit(16).unwrap_or(0))));
    }
    if let Some(oct) = v.strip_prefix("0o").filter(|o| !o.is_empty() && o.chars().all(|c| ('0'..='7').contains(&c))) {
        return number(oct.chars().fold(0.0, |acc, c| acc * 8.0 + f64::from(c.to_digit(8).unwrap_or(0))));
    }
    if CORE_INT.is_match(v) {
        return match v.parse::<i64>() {
            Ok(i) if i.unsigned_abs() < (1 << 53) => Value::from(i),
            _ => number(v.parse::<f64>().unwrap_or(f64::NAN)),
        };
    }
    if CORE_FLOAT.is_match(v) {
        return number(v.parse::<f64>().unwrap_or(f64::NAN));
    }
    Value::String(v.to_string())
}

fn resolve_scalar(v: String, style: TScalarStyle, tag: Option<Tag>) -> Value {
    if let Some(tag) = tag {
        if tag.handle == "tag:yaml.org,2002:" || tag.handle == "!!" {
            return match tag.suffix.as_str() {
                "str" => Value::String(v),
                _ if style != TScalarStyle::Plain => Value::String(v),
                _ => resolve_plain(&v),
            };
        }
        // Non-specific `!` and local tags: a string.
        return Value::String(v);
    }
    if style == TScalarStyle::Plain { resolve_plain(&v) } else { Value::String(v) }
}

/// A mapping key as JavaScript property name (`String(key)`).
fn key_string(key: &Value) -> String {
    match key {
        Value::Null => "null".into(),
        other => ara_prompt::js::to_string(other),
    }
}

enum Node {
    Seq(Vec<Value>, usize),
    Map(Map<String, Value>, Option<Value>, usize),
}

#[derive(Default)]
struct Builder {
    stack: Vec<Node>,
    anchors: std::collections::HashMap<usize, Value>,
    docs: Vec<Value>,
}

impl Builder {
    fn push_value(&mut self, value: Value, anchor: usize) {
        if anchor > 0 {
            self.anchors.insert(anchor, value.clone());
        }
        match self.stack.last_mut() {
            None => self.docs.push(value),
            Some(Node::Seq(items, _)) => items.push(value),
            Some(Node::Map(map, pending, _)) => match pending.take() {
                None => *pending = Some(value),
                Some(key) => {
                    let mergeable = match &value {
                        Value::Object(_) => true,
                        Value::Array(items) => items.iter().all(Value::is_object),
                        _ => false,
                    };
                    if matches!(&key, Value::String(k) if k == "<<") && mergeable {
                        merge_into(map, value);
                    } else {
                        map.insert(key_string(&key), value);
                    }
                }
            },
        }
    }
}

/// `<<` merge: a mapping or a list of mappings; explicit keys win.
fn merge_into(map: &mut Map<String, Value>, value: Value) {
    let sources = match value {
        Value::Array(items) => items,
        other => vec![other],
    };
    for source in sources {
        if let Value::Object(source) = source {
            for (k, v) in source {
                map.entry(k).or_insert(v);
            }
        }
    }
}

impl MarkedEventReceiver for Builder {
    fn on_event(&mut self, event: Event, _mark: Marker) {
        match event {
            Event::Scalar(v, style, anchor, tag) => self.push_value(resolve_scalar(v, style, tag), anchor),
            Event::Alias(id) => {
                let value = self.anchors.get(&id).cloned().unwrap_or(Value::Null);
                self.push_value(value, 0);
            }
            Event::SequenceStart(anchor, _) => self.stack.push(Node::Seq(Vec::new(), anchor)),
            Event::MappingStart(anchor, _) => self.stack.push(Node::Map(Map::new(), None, anchor)),
            Event::SequenceEnd | Event::MappingEnd => match self.stack.pop() {
                Some(Node::Seq(items, anchor)) => self.push_value(Value::Array(items), anchor),
                Some(Node::Map(map, _, anchor)) => self.push_value(Value::Object(js_key_order(map)), anchor),
                None => {}
            },
            _ => {}
        }
    }
}

/// Rebuild a mapping in JS own-key order (integer-like keys first).
fn js_key_order(map: Map<String, Value>) -> Map<String, Value> {
    let order: Vec<String> = ara_prompt::js::entries(&map).into_iter().map(|(k, _)| k.clone()).collect();
    let mut map = map;
    order.into_iter().filter_map(|k| map.remove(&k).map(|v| (k, v))).collect()
}

/// `YAML.parse`: one document → its value; none → null; several → array.
/// Events are pulled one at a time, so nesting never recurses.
pub fn parse_yaml(source: &str) -> Result<Value, String> {
    let mut builder = Builder::default();
    let mut parser = Parser::new_from_str(source);
    let mut depth = 0usize;
    loop {
        let (event, mark) = parser.next_token().map_err(|e| e.to_string())?;
        match event {
            Event::StreamEnd => break,
            Event::SequenceStart(..) | Event::MappingStart(..) => {
                depth += 1;
                if depth > MAX_YAML_DEPTH {
                    return Err(format!("YAML nesting exceeds {MAX_YAML_DEPTH} levels"));
                }
            }
            Event::SequenceEnd | Event::MappingEnd => depth = depth.saturating_sub(1),
            _ => {}
        }
        builder.on_event(event, mark);
    }
    Ok(match builder.docs.len() {
        0 => Value::Null,
        1 => builder.docs.pop().unwrap_or(Value::Null),
        _ => Value::Array(builder.docs),
    })
}

// ---------------------------------------------------------------------------
// frontmatter.ts
// ---------------------------------------------------------------------------

static HTML_COMMENT: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"(?s)<!--.*?-->").unwrap());
static KEBAB: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"-([a-z])").unwrap());
/// JavaScript regex classes: `\s` (JS whitespace), `\S`, ASCII `\w`, and `.`
/// (anything but a line terminator).
const JS_S: &str = r"[\t\n\x0B\x0C\r \u{A0}\u{1680}\u{2000}-\u{200A}\u{2028}\u{2029}\u{202F}\u{205F}\u{3000}\u{FEFF}]";
const JS_NOT_S: &str =
    r"[^\t\n\x0B\x0C\r \u{A0}\u{1680}\u{2000}-\u{200A}\u{2028}\u{2029}\u{202F}\u{205F}\u{3000}\u{FEFF}]";
const JS_DOT: &str = r"[^\n\r\u{2028}\u{2029}]";

fn js_regex(pattern: &str) -> regex::Regex {
    let pattern =
        pattern.replace(r"\S", JS_NOT_S).replace(r"\s", JS_S).replace(r"\w", "[A-Za-z0-9_]").replace("<DOT>", JS_DOT);
    regex::Regex::new(&pattern).unwrap()
}

static PLAIN_SCALAR_KEY_VALUE: LazyLock<regex::Regex> =
    LazyLock::new(|| js_regex(r"^(\s*[A-Za-z_][\w-]*:\s+)(\S<DOT>*?)(\s*)$"));
static FALLBACK_LINE: LazyLock<regex::Regex> = LazyLock::new(|| js_regex(r"^([\w-]+):\s*(<DOT>*)$"));

fn kebab_to_camel(key: &str) -> String {
    if !key.contains('-') {
        return key.to_string();
    }
    KEBAB.replace_all(key, |c: &regex::Captures<'_>| c[1].to_ascii_uppercase()).into_owned()
}

/// Recursively turn kebab-case keys into camelCase (`normalizeFrontmatterKeys`).
pub fn normalize_frontmatter_keys(value: Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.into_iter().map(normalize_frontmatter_keys).collect()),
        Value::Object(map) => {
            Value::Object(map.into_iter().map(|(k, v)| (kebab_to_camel(&k), normalize_frontmatter_keys(v))).collect())
        }
        other => other,
    }
}

/// Quote plain values that contain `: ` (`description: a: b`).
fn quote_ambiguous_plain_scalars(metadata: &str) -> Option<String> {
    let mut changed = false;
    let lines: Vec<String> = metadata
        .split('\n')
        .map(|line| {
            let Some(caps) = PLAIN_SCALAR_KEY_VALUE.captures(line) else { return line.to_string() };
            let value = ara_prompt::js::trim_end(&caps[2]);
            if !value.contains(": ") || value.starts_with(['"', '\'', '[', '{', '|', '>', '!', '&', '*', '#']) {
                return line.to_string();
            }
            changed = true;
            format!("{}{}{}", &caps[1], Value::String(value.to_string()), &caps[3])
        })
        .collect();
    changed.then(|| lines.join("\n"))
}

/// `parseYamlRecord`: the document if it is a mapping.
fn parse_yaml_record(metadata: &str, repair_tabs: bool) -> Result<Option<Map<String, Value>>, String> {
    let source = if repair_tabs { metadata.replace('\t', "  ") } else { metadata.to_string() };
    Ok(match parse_yaml(&source)? {
        Value::Object(map) => Some(map),
        _ => None,
    })
}

/// Parse `---`-delimited YAML frontmatter; the body is the trimmed rest.
pub fn parse_frontmatter(content: &str, options: &FrontmatterOptions) -> Result<Frontmatter, String> {
    let finalize = |fm: Map<String, Value>| -> Map<String, Value> {
        if options.raw_keys {
            fm
        } else {
            match normalize_frontmatter_keys(Value::Object(fm)) {
                Value::Object(map) => map,
                _ => Map::new(),
            }
        }
    };
    let mut frontmatter = options.fallback.clone();
    let newline_normalized =
        if options.normalize { content.replace("\r\n", "\n").replace('\r', "\n") } else { content.to_string() };
    let normalized = if options.normalize && options.repair {
        HTML_COMMENT.replace_all(&newline_normalized, "").into_owned()
    } else {
        newline_normalized
    };
    let plain = |body: String, fm| Ok(Frontmatter { frontmatter: fm, body, warning: None });
    if !normalized.starts_with("---") {
        return plain(normalized, frontmatter);
    }
    let Some(end) = normalized.get(3..).and_then(|rest| rest.find("\n---")).map(|i| i + 3) else {
        return plain(normalized, frontmatter);
    };
    // `slice(4, end)`: skip `---` and the character after it.
    let after_dashes = &normalized[3..end];
    let metadata = after_dashes.char_indices().nth(1).map_or("", |(i, _)| &after_dashes[i..]);
    let body = ara_prompt::js::trim(&normalized[end + 4..]).to_string();

    let error = match parse_yaml_record(metadata, options.repair) {
        Ok(loaded) => {
            frontmatter.extend(loaded.unwrap_or_default());
            return Ok(Frontmatter { frontmatter: finalize(frontmatter), body, warning: None });
        }
        Err(error) => error,
    };
    if options.repair
        && let Some(quoted) = quote_ambiguous_plain_scalars(metadata)
        && let Ok(loaded) = parse_yaml_record(&quoted, true)
    {
        frontmatter.extend(loaded.unwrap_or_default());
        return Ok(Frontmatter { frontmatter: finalize(frontmatter), body, warning: None });
    }
    let source = options.source.clone().unwrap_or_else(|| {
        // `truncate(content, 64)`: 63 UTF-16 units and an ellipsis.
        let units: Vec<u16> = content.encode_utf16().collect();
        let head = if units.len() <= 64 {
            content.to_string()
        } else {
            format!("{}…", String::from_utf16_lossy(&units[..63]))
        };
        format!("Inline '{head}'")
    });
    let message = format!("Failed to parse YAML frontmatter ({source}): {error}");
    if options.level == FailureLevel::Fatal {
        return Err(message);
    }
    // Simple `key: value` fallback, each value reparsed on its own.
    for line in metadata.split('\n') {
        let Some(caps) = FALLBACK_LINE.captures(line) else { continue };
        let raw = ara_prompt::js::trim(&caps[2]).to_string();
        let mut value = Value::String(raw.clone());
        if !raw.is_empty()
            && let Ok(parsed) = parse_yaml(&raw)
            && !parsed.is_null()
            && !parsed.is_object()
        {
            value = parsed;
        }
        frontmatter.insert(caps[1].to_string(), value);
    }
    let warning = (options.level == FailureLevel::Warn).then_some(message);
    Ok(Frontmatter { frontmatter: finalize(frontmatter), body, warning })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Expected values: `Bun.YAML.parse` (YAML 1.2 core schema).
    #[test]
    fn core_schema_resolution() {
        let v = parse_yaml("a: Null\nb: NULL\nc: ~\nd:\ne: null").unwrap();
        assert_eq!(v, json!({"a": null, "b": null, "c": null, "d": null, "e": null}));
        assert_eq!(parse_yaml("a: 1\na: 2").unwrap(), json!({"a": 2}));
        assert_eq!(parse_yaml("a: !!str 1\nb: !!int '7'\nc: ! 3").unwrap(), json!({"a": "1", "b": "7", "c": "3"}));
        assert_eq!(
            parse_yaml("base: &b {x: 1}\nm:\n  <<: *b\n  y: 2").unwrap(),
            json!({"base": {"x": 1}, "m": {"x": 1, "y": 2}})
        );
        let n = parse_yaml(
            "big: 12345678901234567890\nf: 1e3\nj: 0x1F\nk: 0o17\nl: +5\nm: 1.\nn: .5\no: 1_000\ng: .inf\ni: .NaN",
        )
        .unwrap();
        assert_eq!(n["big"].as_f64(), Some(12345678901234567000.0));
        assert_eq!(
            (n["f"].clone(), n["j"].clone(), n["k"].clone(), n["l"].clone()),
            (json!(1000), json!(31), json!(15), json!(5))
        );
        assert_eq!((n["m"].clone(), n["n"].clone(), n["o"].clone()), (json!(1), json!(0.5), json!("1_000")));
        // Non-finite: NaN is null (falsy), Infinity a truthy number.
        assert_eq!((n["g"].as_f64(), n["i"].clone()), (Some(f64::MAX), json!(null)));
        assert_eq!(parse_yaml("? [a, b]\n: c\n1: x\ntrue: y").unwrap(), json!({"a,b": "c", "1": "x", "true": "y"}));
        assert_eq!(parse_yaml("- a\n- b").unwrap(), json!(["a", "b"]));
        assert_eq!(parse_yaml("a: 1\n---\nb: 2").unwrap(), json!([{"a": 1}, {"b": 2}]));
        assert!(parse_yaml("a: [1, 2\n").is_err());
        assert_eq!(
            parse_yaml("t: yes\nu: on\nv: 2024-01-01\nw: 0b101").unwrap(),
            json!({"t": "yes", "u": "on", "v": "2024-01-01", "w": "0b101"})
        );
    }
}
