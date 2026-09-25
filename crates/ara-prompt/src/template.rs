//! Handlebars-compatible template engine (OMP `packages/utils/src/template.ts`
//! at 596f2da7101178214aa27a753529d15e6b7ad91d, itself a behavior-compatible
//! reimplementation of the handlebars surface OMP prompts use).
//!
//! Ported: standalone-tag stripping, `{{!…}}`/`{{!--…--}}` comments,
//! `{{expr}}` (escaped), `{{{expr}}}`/`{{&expr}}` (raw), `{{#block}}` /
//! `{{^inverted}}` / `{{else}}` / `{{else if …}}` chains, `{{> partial}}`,
//! subexpressions, hash arguments, string/number/boolean/null literals,
//! dotted/bracket/`this`/`../`/`@root`/`@data` paths, the built-in `if`,
//! `unless`, `each` (arrays and objects, `@index`/`@key`/`@first`/`@last`),
//! `with` and `lookup`, helper-over-property precedence, and the exact
//! escape entity set. Contexts are `serde_json::Value` (object keys keep
//! insertion order); `Null` stands for JS `undefined` (see `js`).
//!
//! Intentional difference: a `{{#if}}…{{else if}}…{{/if}}` chain closes with
//! one `{{/if}}`, as in Handlebars. Upstream's parser leaves the outer block
//! open and reports "Parse error: unclosed block if"; no upstream prompt uses
//! `else if`. The double-closed form upstream accepts
//! (`{{#if a}}…{{else if b}}…{{/if}}{{/if}}`) is a parse error here.
//!
//! Intentional difference: nesting (blocks, subexpressions, partials) is
//! capped at [`MAX_NESTING`] levels and fails with a [`TemplateError`];
//! upstream recurses until the JS stack overflows (~10^5 levels).
//!
//! Partials render like upstream's string partials: through a registry
//! holding only the built-ins (upstream compiles them with the module-global
//! registry, which prompt code never populates), so helpers and other
//! partials are not visible inside a partial.

use crate::js;
use serde_json::{Map, Value};
use std::borrow::Cow;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, LazyLock};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateError(pub String);

impl std::fmt::Display for TemplateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TemplateError {}

type Result<T> = std::result::Result<T, TemplateError>;

fn err<T>(message: impl Into<String>) -> Result<T> {
    Err(TemplateError(message.into()))
}

/// A helper's return value.
#[derive(Debug, Clone, PartialEq)]
pub enum Output {
    Value(Value),
    /// Bypasses HTML escaping (`SafeString`).
    Safe(String),
}

impl From<Value> for Output {
    fn from(v: Value) -> Self {
        Output::Value(v)
    }
}

impl From<String> for Output {
    fn from(s: String) -> Self {
        Output::Value(Value::String(s))
    }
}

impl From<&str> for Output {
    fn from(s: &str) -> Self {
        Output::Value(Value::String(s.into()))
    }
}

impl From<bool> for Output {
    fn from(b: bool) -> Self {
        Output::Value(Value::Bool(b))
    }
}

pub type Helper = Arc<dyn Fn(&HelperCall<'_>) -> Result<Output> + Send + Sync>;

#[derive(Debug, Clone)]
enum Expr {
    Path(String),
    Literal(Value),
    Call(Call),
}

#[derive(Debug, Clone)]
struct Call {
    name: String,
    args: Vec<Expr>,
    hash: Vec<(String, Expr)>,
}

#[derive(Debug, Clone)]
enum Node {
    Text(String),
    Output { call: Call, escaped: bool },
    Block { call: Call, body: Vec<Node>, inverse: Vec<Node> },
    Partial(Call),
}

struct Frame {
    context: Rc<Value>,
    parents: Vec<Rc<Frame>>,
    root: Rc<Value>,
    data: Rc<Map<String, Value>>,
}

/// Arguments and block access handed to a helper.
pub struct HelperCall<'a> {
    pub name: &'a str,
    pub args: Vec<Value>,
    pub hash: Map<String, Value>,
    body: &'a [Node],
    inverse: &'a [Node],
    frame: &'a Rc<Frame>,
    engine: &'a Engine,
    depth: usize,
}

impl HelperCall<'_> {
    /// The current context (`this`).
    pub fn this(&self) -> &Value {
        &self.frame.context
    }

    pub fn arg(&self, index: usize) -> &Value {
        self.args.get(index).unwrap_or(&Value::Null)
    }

    /// Render the block body with `context` (`options.fn`).
    pub fn render(&self, context: &Value) -> Result<String> {
        let frame = child(self.frame, Rc::new(context.clone()), None);
        self.engine.render_nodes(self.body, &frame, self.depth + 1)
    }

    /// Render the `{{else}}` part with `context` (`options.inverse`).
    pub fn inverse(&self, context: &Value) -> Result<String> {
        let frame = child(self.frame, Rc::new(context.clone()), None);
        self.engine.render_nodes(self.inverse, &frame, self.depth + 1)
    }
}

fn child(frame: &Rc<Frame>, context: Rc<Value>, data: Option<Map<String, Value>>) -> Rc<Frame> {
    let mut parents = Vec::with_capacity(frame.parents.len() + 1);
    parents.push(Rc::clone(frame));
    parents.extend(frame.parents.iter().cloned());
    Rc::new(Frame {
        context,
        parents,
        root: Rc::clone(&frame.root),
        data: data.map_or_else(|| Rc::clone(&frame.data), Rc::new),
    })
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Maximum nesting of blocks, subexpressions and partials.
pub const MAX_NESTING: usize = 100;

/// JavaScript's `\s` set, for regexes that must not use Rust's Unicode `\s`.
const JS_SPACE: &str =
    r"\t\n\x0B\x0C\r \u{A0}\u{1680}\u{2000}-\u{200A}\u{2028}\u{2029}\u{202F}\u{205F}\u{3000}\u{FEFF}";

fn js_regex(pattern: &str) -> regex::Regex {
    regex::Regex::new(&pattern.replace("\\s", &format!("[{JS_SPACE}]"))).unwrap()
}

static BLOCK_TAG: LazyLock<regex::Regex> = LazyLock::new(|| js_regex(r"^\s*\{\{(?:#|/|\^|else(?-u:\b)|!)"));
static STANDALONE_TAG: LazyLock<regex::Regex> =
    LazyLock::new(|| js_regex(r"^\s*\{\{(?:#|/|\^|else(?-u:\b)|!)[^{}]*\}\}\s*$"));
static COMMENT_OPEN: LazyLock<regex::Regex> = LazyLock::new(|| js_regex(r"^\s*\{\{!--"));
static NUMBER: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^-?(?:[0-9]+\.?[0-9]*|\.[0-9]+)$").unwrap());

fn line_body(line: &str) -> &str {
    match line.strip_suffix('\n') {
        Some(body) => body.strip_suffix('\r').unwrap_or(body),
        None => line,
    }
}

/// Lines holding only a block tag or comment lose their surrounding
/// whitespace and newline (`stripStandalone`).
fn strip_standalone(source: &str) -> String {
    let mut lines: Vec<String> = source.split_inclusive('\n').map(str::to_string).collect();
    let mut index = 0;
    while index < lines.len() {
        let body = line_body(&lines[index]).to_string();
        if COMMENT_OPEN.is_match(&body) && !body.contains("--}}") {
            for end in index + 1..lines.len() {
                let end_line = line_body(&lines[end]).to_string();
                let Some(close) = end_line.find("--}}") else { continue };
                if end_line[close + 4..].chars().all(js::is_space) {
                    for line in lines.iter_mut().take(end + 1).skip(index) {
                        line.clear();
                    }
                    index = end;
                }
                break;
            }
            index += 1;
            continue;
        }
        if BLOCK_TAG.is_match(&body) && STANDALONE_TAG.is_match(&body) {
            lines[index] = js::trim(&body).to_string();
        }
        index += 1;
    }
    lines.concat()
}

fn tokenize(source: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut start: Option<usize> = None;
    let mut quote: Option<char> = None;
    let (mut depth, mut brackets) = (0i32, 0i32);
    let mut chars = source.char_indices().peekable();
    while let Some((index, c)) = chars.next() {
        if let Some(q) = quote {
            if c == '\\' {
                chars.next();
            } else if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => {
                start.get_or_insert(index);
                quote = Some(c);
            }
            '(' => {
                start.get_or_insert(index);
                depth += 1;
            }
            ')' => depth -= 1,
            '[' => {
                start.get_or_insert(index);
                brackets += 1;
            }
            ']' => brackets -= 1,
            c if js::is_space(c) && depth == 0 && brackets == 0 => {
                if let Some(s) = start.take() {
                    tokens.push(&source[s..index]);
                }
            }
            _ => {
                start.get_or_insert(index);
            }
        }
    }
    if let Some(s) = start {
        tokens.push(&source[s..]);
    }
    tokens
}

fn parse_string(token: &str) -> String {
    let quote = token.chars().next().unwrap_or('"');
    let inner = &token[quote.len_utf8()..token.len() - quote.len_utf8()];
    let mut out = String::new();
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\'
            && let Some(&next) = chars.peek()
        {
            chars.next();
            if next == quote || next == '\\' {
                out.push(next);
            } else {
                out.push('\\');
                out.push(next);
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn parse_atom(token: &str, depth: usize) -> Result<Expr> {
    if token.len() >= 2 && token.starts_with('(') && token.ends_with(')') {
        return Ok(Expr::Call(parse_call(&token[1..token.len() - 1], depth + 1)?));
    }
    if (token.starts_with('"') && token.ends_with('"')) || (token.starts_with('\'') && token.ends_with('\'')) {
        return Ok(Expr::Literal(Value::String(if token.len() < 2 { String::new() } else { parse_string(token) })));
    }
    Ok(match token {
        "true" => Expr::Literal(Value::Bool(true)),
        "false" => Expr::Literal(Value::Bool(false)),
        "null" | "undefined" => Expr::Literal(Value::Null),
        t if NUMBER.is_match(t) => Expr::Literal(js::number(t.parse::<f64>().unwrap_or(f64::NAN))),
        t => Expr::Path(t.to_string()),
    })
}

fn hash_separator(token: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    let mut depth = 0i32;
    let mut chars = token.char_indices();
    while let Some((index, c)) = chars.next() {
        if let Some(q) = quote {
            if c == '\\' {
                chars.next();
            } else if c == q {
                quote = None;
            }
        } else if c == '"' || c == '\'' {
            quote = Some(c);
        } else if c == '(' {
            depth += 1;
        } else if c == ')' {
            depth -= 1;
        } else if c == '=' && depth == 0 {
            return Some(index);
        }
    }
    None
}

fn parse_call(source: &str, depth: usize) -> Result<Call> {
    if depth > MAX_NESTING {
        return err(nesting_error());
    }
    let mut tokens = tokenize(js::trim(source)).into_iter();
    let name = tokens.next().unwrap_or_default().to_string();
    let mut args = Vec::new();
    let mut hash = Vec::new();
    for token in tokens {
        match hash_separator(token).filter(|&i| i > 0) {
            Some(i) => hash.push((token[..i].to_string(), parse_atom(&token[i + 1..], depth)?)),
            None => args.push(parse_atom(token, depth)?),
        }
    }
    Ok(Call { name, args, hash })
}

fn nesting_error() -> String {
    format!("Template nesting exceeds {MAX_NESTING} levels")
}

fn find_tag_end(source: &str, start: usize, triple: bool) -> Result<usize> {
    let close = if triple { "}}}" } else { "}}" };
    let mut quote: Option<char> = None;
    let mut depth = 0i32;
    let mut chars = source[start..].char_indices();
    while let Some((offset, c)) = chars.next() {
        let index = start + offset;
        if let Some(q) = quote {
            if c == '\\' {
                chars.next();
            } else if c == q {
                quote = None;
            }
            continue;
        }
        if c == '"' || c == '\'' {
            quote = Some(c);
        } else if c == '(' {
            depth += 1;
        } else if c == ')' {
            depth -= 1;
        } else if depth == 0 && source[index..].starts_with(close) {
            return Ok(index);
        }
    }
    err("Parse error: unclosed template expression")
}

struct Open {
    call: Call,
    body: Vec<Node>,
    inverse: Vec<Node>,
    in_inverse: bool,
    inverted: bool,
    /// Pushed by `{{else if …}}`: closes together with its parent.
    chained: bool,
}

fn target<'a>(root: &'a mut Vec<Node>, stack: &'a mut [Open]) -> &'a mut Vec<Node> {
    match stack.last_mut() {
        Some(open) if open.in_inverse => &mut open.inverse,
        Some(open) => &mut open.body,
        None => root,
    }
}

fn close_block(stack: &mut Vec<Open>, root: &mut Vec<Node>) {
    let open = stack.pop().expect("open block");
    let (body, inverse) = if open.inverted { (open.inverse, open.body) } else { (open.body, open.inverse) };
    let node = Node::Block { call: open.call, body, inverse };
    target(root, stack).push(node);
}

fn parse_template(source: &str) -> Result<Vec<Node>> {
    let mut root: Vec<Node> = Vec::new();
    let mut stack: Vec<Open> = Vec::new();
    let mut cursor = 0;
    while cursor < source.len() {
        let Some(rel) = source[cursor..].find("{{") else {
            target(&mut root, &mut stack).push(Node::Text(source[cursor..].to_string()));
            break;
        };
        let open = cursor + rel;
        if open > cursor {
            target(&mut root, &mut stack).push(Node::Text(source[cursor..open].to_string()));
        }
        if source[open..].starts_with("{{!--") {
            // Upstream searches from `open + 6`, one past the opener, so `{{!----}}` is unclosed.
            let from = open + 6;
            let Some(end) = source.as_bytes().get(from..).and_then(|rest| rest.windows(4).position(|w| w == b"--}}"))
            else {
                return err("Parse error: unclosed comment");
            };
            cursor = from + end + 4;
            continue;
        }
        let triple = source[open..].starts_with("{{{");
        let content_start = open + if triple { 3 } else { 2 };
        let end = find_tag_end(source, content_start, triple)?;
        let raw = js::trim(&source[content_start..end]);
        cursor = end + if triple { 3 } else { 2 };
        if raw.is_empty() || raw.starts_with('!') {
            continue;
        }
        if raw == "else" || raw.starts_with("else ") {
            let Some(current) = stack.last_mut() else { return err("Parse error: unexpected else") };
            current.in_inverse = true;
            if raw.len() > 4 {
                if stack.len() >= MAX_NESTING {
                    return err(nesting_error());
                }
                stack.push(Open {
                    call: parse_call(&raw[5..], 0)?,
                    body: Vec::new(),
                    inverse: Vec::new(),
                    in_inverse: false,
                    inverted: false,
                    chained: true,
                });
            }
            continue;
        }
        if let Some(rest) = raw.strip_prefix('#').or_else(|| raw.strip_prefix('^')) {
            if stack.len() >= MAX_NESTING {
                return err(nesting_error());
            }
            stack.push(Open {
                call: parse_call(rest, 0)?,
                body: Vec::new(),
                inverse: Vec::new(),
                in_inverse: false,
                inverted: raw.starts_with('^'),
                chained: false,
            });
            continue;
        }
        if let Some(name) = raw.strip_prefix('/') {
            // `{{else if}}` blocks close with the block that opened the chain.
            while stack.last().is_some_and(|o| o.chained) {
                close_block(&mut stack, &mut root);
            }
            match stack.last() {
                Some(current) if current.call.name == js::trim(name) => close_block(&mut stack, &mut root),
                _ => return err(format!("Parse error: mismatched {raw}")),
            }
            continue;
        }
        let node = if let Some(rest) = raw.strip_prefix('>') {
            Node::Partial(parse_call(rest, 0)?)
        } else {
            let (expr, amp) = match raw.strip_prefix('&') {
                Some(rest) => (rest, true),
                None => (raw, false),
            };
            Node::Output { call: parse_call(expr, 0)?, escaped: !triple && !amp }
        };
        target(&mut root, &mut stack).push(node);
    }
    if let Some(open) = stack.iter().rev().find(|o| !o.chained).or(stack.last()) {
        return err(format!("Parse error: unclosed block {}", open.call.name));
    }
    Ok(root)
}

// ---------------------------------------------------------------------------
// Evaluation
// ---------------------------------------------------------------------------

fn path_parts(path: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut bracket: Option<usize> = None;
    let bytes: Vec<(usize, char)> = path.char_indices().collect();
    let mut i = 0;
    while i <= bytes.len() {
        let (index, c) = bytes.get(i).copied().map_or((path.len(), None), |(ix, c)| (ix, Some(c)));
        match (c, bracket) {
            (Some('['), None) => {
                if index > start {
                    parts.push(path[start..index].trim_end_matches(['.', '/']).to_string());
                }
                bracket = Some(index + 1);
            }
            (Some(']'), Some(b)) => {
                parts.push(path[b..index].to_string());
                start = index + 1;
                bracket = None;
            }
            (Some('.') | Some('/') | None, None) => {
                if index > start {
                    parts.push(path[start..index].to_string());
                }
                start = index + 1;
            }
            _ => {}
        }
        i += 1;
    }
    parts.into_iter().filter(|p| !p.is_empty()).collect()
}

/// Canonical array index (`Object.hasOwn(array, key)`): `"1"`, not `"01"` or `"+1"`.
fn array_index(key: &str) -> Option<usize> {
    key.parse::<usize>().ok().filter(|i| i.to_string() == key)
}

/// Proto-safe own-property lookup (upstream `property`), by reference.
fn property_ref<'v>(parent: &'v Value, key: &str) -> Cow<'v, Value> {
    let missing = || Cow::Owned(Value::Null);
    match parent {
        Value::String(s) if key == "length" => Cow::Owned(js::number(js::utf16_len(s) as f64)),
        Value::Array(items) if key == "length" => Cow::Owned(js::number(items.len() as f64)),
        Value::Array(items) => array_index(key).and_then(|i| items.get(i)).map_or_else(missing, Cow::Borrowed),
        Value::Object(map) if !matches!(key, "__proto__" | "prototype" | "constructor") => {
            map.get(key).map_or_else(missing, Cow::Borrowed)
        }
        _ => missing(),
    }
}

pub(crate) fn property(parent: &Value, key: &str) -> Value {
    property_ref(parent, key).into_owned()
}

/// Follow `parts` from `start`, cloning only the value reached.
fn walk<S: AsRef<str>>(start: &Value, parts: &[S]) -> Value {
    let mut current = Cow::Borrowed(start);
    for part in parts {
        current = match current {
            Cow::Borrowed(v) => property_ref(v, part.as_ref()),
            Cow::Owned(v) => Cow::Owned(property(&v, part.as_ref())),
        };
    }
    current.into_owned()
}

fn resolve_path(path: &str, frame: &Rc<Frame>) -> Value {
    if path == "this" || path == "." {
        return (*frame.context).clone();
    }
    let mut current = Rc::clone(frame);
    let mut path = path;
    while let Some(rest) = path.strip_prefix("../") {
        if let Some(parent) = current.parents.first() {
            current = Rc::clone(parent);
        }
        path = rest;
    }
    if path == "this" || path == "." {
        return (*current.context).clone();
    }
    let path = path.strip_prefix("this.").unwrap_or(path);
    if path == "@root" {
        return (*current.root).clone();
    }
    if let Some(rest) = path.strip_prefix("@root.") {
        return walk(&current.root, &path_parts(rest));
    }
    if let Some(rest) = path.strip_prefix('@') {
        let parts = path_parts(rest);
        let Some((first, rest)) = parts.split_first() else { return Value::Null };
        return current.data.get(first).map_or(Value::Null, |start| walk(start, rest));
    }
    walk(&current.context, &path_parts(path))
}

fn conditional_truthy(value: &Value, include_zero: bool) -> bool {
    match value {
        Value::Number(n) if n.as_f64() == Some(0.0) => include_zero,
        Value::Array(items) => !items.is_empty(),
        other => js::truthy(other),
    }
}

/// Handlebars' HTML escape entity set.
pub fn escape_expression(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            '`' => out.push_str("&#x60;"),
            '=' => out.push_str("&#x3D;"),
            c => out.push(c),
        }
    }
    out
}

fn stringify(output: Output, escape: bool) -> String {
    match output {
        Output::Safe(s) => s,
        Output::Value(Value::Null) => String::new(),
        Output::Value(v) => {
            let text = js::to_string(&v);
            if escape { escape_expression(&text) } else { text }
        }
    }
}

/// Template compilation behavior (`CompileOptions`).
#[derive(Clone, Copy, Debug, Default)]
pub struct CompileOptions {
    pub no_escape: bool,
}

/// A helper and partial registry (`TemplateEngine`).
#[derive(Clone, Default)]
pub struct Engine {
    helpers: HashMap<String, Helper>,
    partials: HashMap<String, String>,
    options: CompileOptions,
}

/// A parsed template.
#[derive(Debug, Clone)]
pub struct Template {
    nodes: Vec<Node>,
}

impl Engine {
    pub fn new() -> Self {
        Engine::default()
    }

    pub fn with_options(options: CompileOptions) -> Self {
        Engine { options, ..Engine::default() }
    }

    pub fn register_helper(
        &mut self,
        name: &str,
        helper: impl Fn(&HelperCall<'_>) -> Result<Output> + Send + Sync + 'static,
    ) {
        self.helpers.insert(name.to_string(), Arc::new(helper));
    }

    pub fn register_partial(&mut self, name: &str, source: &str) {
        self.partials.insert(name.to_string(), source.to_string());
    }

    pub fn compile(&self, source: &str) -> Result<Template> {
        Ok(Template { nodes: parse_template(&strip_standalone(source))? })
    }

    /// Compile and render in one step.
    pub fn render(&self, source: &str, context: &Value) -> Result<String> {
        let template = self.compile(source)?;
        self.render_template(&template, context)
    }

    pub fn render_template(&self, template: &Template, context: &Value) -> Result<String> {
        // `context ?? {}`
        let root = Rc::new(if context.is_null() { Value::Object(Map::new()) } else { context.clone() });
        // `@root` resolves through `Frame::root`; the data map holds only
        // iteration variables so `each` copies stay small.
        let frame = Rc::new(Frame { context: Rc::clone(&root), parents: Vec::new(), root, data: Rc::new(Map::new()) });
        self.render_nodes(&template.nodes, &frame, 0)
    }

    fn render_nodes(&self, nodes: &[Node], frame: &Rc<Frame>, depth: usize) -> Result<String> {
        if depth > MAX_NESTING {
            return err(nesting_error());
        }
        let mut out = String::new();
        for node in nodes {
            match node {
                Node::Text(t) => out.push_str(t),
                Node::Output { call, escaped } => {
                    let value = self.eval_call(call, frame, false, &[], &[], depth)?;
                    out.push_str(&stringify(value, *escaped && !self.options.no_escape));
                }
                Node::Partial(call) => out.push_str(&self.render_partial(call, frame, depth)?),
                Node::Block { call, body, inverse } => {
                    out.push_str(&self.eval_block(call, body, inverse, frame, depth)?)
                }
            }
        }
        Ok(out)
    }

    fn eval_expr(&self, expr: &Expr, frame: &Rc<Frame>, depth: usize) -> Result<Value> {
        match expr {
            Expr::Literal(v) => Ok(v.clone()),
            Expr::Path(p) => Ok(resolve_path(p, frame)),
            Expr::Call(call) => Ok(match self.eval_call(call, frame, false, &[], &[], depth)? {
                Output::Value(v) => v,
                Output::Safe(s) => Value::String(s),
            }),
        }
    }

    fn eval_hash(&self, call: &Call, frame: &Rc<Frame>, depth: usize) -> Result<Map<String, Value>> {
        let mut hash = Map::new();
        for (key, expr) in &call.hash {
            hash.insert(key.clone(), self.eval_expr(expr, frame, depth)?);
        }
        Ok(hash)
    }

    fn eval_call(
        &self,
        call: &Call,
        frame: &Rc<Frame>,
        force_helper: bool,
        body: &[Node],
        inverse: &[Node],
        depth: usize,
    ) -> Result<Output> {
        let args = call.args.iter().map(|a| self.eval_expr(a, frame, depth)).collect::<Result<Vec<_>>>()?;
        let hash = self.eval_hash(call, frame, depth)?;
        if let Some(helper) = self.helpers.get(&call.name) {
            let helper_call = HelperCall { name: &call.name, args, hash, body, inverse, frame, engine: self, depth };
            return helper(&helper_call);
        }
        if call.name == "lookup" && args.len() >= 2 {
            return Ok(Output::Value(property(&args[0], &js::to_string(&args[1]))));
        }
        if force_helper || !args.is_empty() || !hash.is_empty() {
            return err(format!("Missing helper: \"{}\"", call.name));
        }
        Ok(Output::Value(resolve_path(&call.name, frame)))
    }

    fn eval_block(
        &self,
        call: &Call,
        body: &[Node],
        inverse: &[Node],
        frame: &Rc<Frame>,
        depth: usize,
    ) -> Result<String> {
        let name = call.name.as_str();
        if self.helpers.contains_key(name) {
            return Ok(stringify(self.eval_call(call, frame, true, body, inverse, depth)?, false));
        }
        let depth = depth + 1;
        let args = call.args.iter().map(|a| self.eval_expr(a, frame, depth)).collect::<Result<Vec<_>>>()?;
        let value = match args.first() {
            Some(v) => v.clone(),
            None => resolve_path(name, frame),
        };
        let hash = self.eval_hash(call, frame, depth)?;
        match name {
            "if" | "unless" => {
                let truthy = conditional_truthy(&value, hash.get("includeZero") == Some(&Value::Bool(true)));
                let branch = if name == "if" { truthy } else { !truthy };
                self.render_nodes(if branch { body } else { inverse }, frame, depth)
            }
            "each" => {
                let entries: Vec<(Value, Value)> = match &value {
                    Value::Array(items) => {
                        items.iter().enumerate().map(|(i, v)| (js::number(i as f64), v.clone())).collect()
                    }
                    Value::Object(map) => {
                        js::entries(map).into_iter().map(|(k, v)| (Value::String(k.clone()), v.clone())).collect()
                    }
                    _ => Vec::new(),
                };
                if entries.is_empty() {
                    return self.render_nodes(inverse, frame, depth);
                }
                let last = entries.len() - 1;
                let mut out = String::new();
                for (index, (key, item)) in entries.into_iter().enumerate() {
                    let mut data = (*frame.data).clone();
                    data.insert("index".into(), js::number(index as f64));
                    data.insert("key".into(), key);
                    data.insert("first".into(), Value::Bool(index == 0));
                    data.insert("last".into(), Value::Bool(index == last));
                    out.push_str(&self.render_nodes(body, &child(frame, Rc::new(item), Some(data)), depth)?);
                }
                Ok(out)
            }
            "with" => {
                if conditional_truthy(&value, false) {
                    self.render_nodes(body, &child(frame, Rc::new(value), None), depth)
                } else {
                    self.render_nodes(inverse, frame, depth)
                }
            }
            _ => {
                let resolved = resolve_path(name, frame);
                match resolved {
                    Value::Array(items) => {
                        let mut out = String::new();
                        for item in items {
                            out.push_str(&self.render_nodes(body, &child(frame, Rc::new(item), None), depth)?);
                        }
                        if out.is_empty() { self.render_nodes(inverse, frame, depth) } else { Ok(out) }
                    }
                    Value::Bool(false) | Value::Null => self.render_nodes(inverse, frame, depth),
                    other => self.render_nodes(body, &child(frame, Rc::new(other), None), depth),
                }
            }
        }
    }

    fn render_partial(&self, call: &Call, frame: &Rc<Frame>, depth: usize) -> Result<String> {
        let Some(source) = self.partials.get(&call.name) else {
            return err(format!("The partial {} could not be found", call.name));
        };
        let context = match call.args.first() {
            Some(arg) => self.eval_expr(arg, frame, depth)?,
            None => (*frame.context).clone(),
        };
        let hash = self.eval_hash(call, frame, depth)?;
        // `{ ...context, ...hash }` for any object context (an array spreads its indices).
        let merged = match context {
            Value::Object(mut map) => {
                map.extend(hash);
                Value::Object(map)
            }
            Value::Array(items) => {
                let mut map: Map<String, Value> =
                    items.into_iter().enumerate().map(|(i, v)| (i.to_string(), v)).collect();
                map.extend(hash);
                Value::Object(map)
            }
            other => other,
        };
        // Upstream compiles string partials with the module-global registry:
        // built-ins only.
        let inner = Engine::with_options(self.options);
        let template = inner.compile(source)?;
        let root = Rc::new(if merged.is_null() { Value::Object(Map::new()) } else { merged });
        let frame = Rc::new(Frame { context: Rc::clone(&root), parents: Vec::new(), root, data: Rc::new(Map::new()) });
        inner.render_nodes(&template.nodes, &frame, depth + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn path_parts_split_dots_slashes_and_brackets() {
        assert_eq!(path_parts("user.[display name]"), ["user", "display name"]);
        assert_eq!(path_parts("[literal.key]"), ["literal.key"]);
        assert_eq!(path_parts("a/b.c"), ["a", "b", "c"]);
    }

    #[test]
    fn standalone_lines_lose_their_newline() {
        assert_eq!(strip_standalone("a\n  {{#if x}}  \nb\n{{/if}}\nc"), "a\n{{#if x}}b\n{{/if}}c");
    }

    #[test]
    fn else_if_chains() {
        let e = Engine::new();
        let t = "{{#if a}}A{{else if b}}B{{else}}C{{/if}}";
        assert_eq!(e.render(t, &json!({"a": true})).unwrap(), "A");
        assert_eq!(e.render(t, &json!({"b": true})).unwrap(), "B");
        assert_eq!(e.render(t, &json!({})).unwrap(), "C");
        assert_eq!(e.render("{{^if a}}no{{else}}yes{{/if}}", &json!({"a": true})).unwrap(), "yes");
        assert!(e.render("{{#if a}}x{{/each}}", &json!({})).unwrap_err().0.starts_with("Parse error: mismatched"));
        assert_eq!(e.render("{{#if a}}x", &json!({})).unwrap_err().0, "Parse error: unclosed block if");
    }
}
