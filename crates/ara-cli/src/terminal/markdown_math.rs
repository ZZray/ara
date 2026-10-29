//! Fixed OMP 596f2da math delimiter grammar and glyph-first LaTeX renderer.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
#[path = "markdown_css_color.rs"]
mod css;
#[path = "markdown_math_layout.rs"]
mod layout;
#[path = "markdown_math_tables.rs"]
mod tables;
pub fn latex_to_block(source: &str) -> Vec<String> {
    layout::latex_to_block(source)
}
pub fn latex_to_block_with_color(source: &str, true_color: bool) -> Vec<String> {
    layout::latex_to_block_with_color(source, true_color)
}
use tables::*;

/// Source positions use UTF-16 units, matching the fixed delimiter contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MathSpan {
    pub opener: &'static str,
    pub display: bool,
    pub end: usize,
    pub body: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MathBlock {
    pub raw: String,
    pub body: String,
}
fn escaped_at(s: &[u16], at: usize, from: usize) -> bool {
    let mut n = 0;
    let mut p = at;
    while p > from && s[p - 1] == 92 {
        n += 1;
        p -= 1;
    }
    n % 2 == 1
}
pub fn math_start_index(source: &str, from: usize) -> Option<usize> {
    let s: Vec<u16> = source.encode_utf16().collect();
    (from..s.len()).find(|&p| s[p] == 36 || s[p] == 92 && matches!(s.get(p + 1), Some(40 | 91)))
}
pub fn math_opener_at(source: &str, at: usize) -> Option<&'static str> {
    opener_at(&source.encode_utf16().collect::<Vec<_>>(), at)
}
fn opener_at(s: &[u16], at: usize) -> Option<&'static str> {
    match s.get(at) {
        Some(36) => Some(if s.get(at + 1) == Some(&36) { "$$" } else { "$" }),
        Some(92) => match s.get(at + 1) {
            Some(40) => Some("\\("),
            Some(91) => Some("\\["),
            _ => None,
        },
        _ => None,
    }
}
pub fn math_span_at(source: &str, at: usize, from: usize) -> Option<MathSpan> {
    let s: Vec<u16> = source.encode_utf16().collect();
    let opener = opener_at(&s, at)?;
    if escaped_at(&s, at, from) {
        return None;
    }
    let start = at + opener.len();
    let close = if opener == "$" {
        if matches!(s.get(start), None | Some(32 | 9 | 10 | 36)) {
            return None;
        }
        let mut p = start;
        let mut found = None;
        while p < s.len() {
            match s[p] {
                92 => p += 2,
                10 => return None,
                36 => {
                    if matches!(s.get(p.wrapping_sub(1)), Some(32 | 9)) {
                        return None;
                    }
                    if s.get(p + 1).is_some_and(|u| (48..=57).contains(u)) {
                        p += 1;
                        continue;
                    }
                    if String::from_utf16_lossy(&s[start..p]).trim().is_empty() {
                        return None;
                    }
                    found = Some(p);
                    break;
                }
                _ => p += 1,
            }
        }
        found?
    } else {
        let closer: Vec<u16> = match opener {
            "\\(" => "\\)",
            "\\[" => "\\]",
            _ => opener,
        }
        .encode_utf16()
        .collect();
        (start..s.len()).find(|&p| s[p..].starts_with(&closer) && !escaped_at(&s, p, start))?
    };
    let body = String::from_utf16_lossy(&s[start..close]);
    if opener == "$$" && body.trim().is_empty() {
        return None;
    }
    Some(MathSpan { opener, display: matches!(opener, "$$" | "\\["), end: close + opener.len(), body })
}
pub fn math_block_at(source: &str) -> Option<MathBlock> {
    fn delimiter(line: &str) -> Option<&str> {
        let spaces = line.chars().take_while(|&c| c == ' ').count();
        if spaces > 3 {
            return None;
        }
        Some(line[spaces..].trim_end_matches([' ', '\t', '\r']))
    }
    let newline = source.find('\n')?;
    let opening = delimiter(&source[..newline])?;
    let closing = match opening {
        "$$" => "$$",
        "\\[" => "\\]",
        _ => return None,
    };
    let body_start = newline + 1;
    let mut line_start = body_start;
    while line_start < source.len() {
        let end = source[line_start..].find('\n').map(|n| line_start + n).unwrap_or(source.len());
        if delimiter(&source[line_start..end]) == Some(closing) {
            let mut body_end = line_start;
            if body_end > body_start && source.as_bytes()[body_end - 1] == b'\n' {
                body_end -= 1;
                if body_end > body_start && source.as_bytes()[body_end - 1] == b'\r' {
                    body_end -= 1;
                }
            }
            let body = &source[body_start..body_end];
            if body.is_empty() {
                if end == source.len() {
                    return None;
                }
                line_start = end + 1;
                continue;
            }
            if body.trim().is_empty() {
                return None;
            }
            let raw_end = if end < source.len() { end + 1 } else { end };
            return Some(MathBlock { raw: source[..raw_end].into(), body: body.into() });
        }
        if end == source.len() {
            break;
        }
        line_start = end + 1;
    }
    None
}

#[derive(Clone, Default)]
struct ParseStyle {
    font: Option<&'static str>,
    bold: bool,
    italic: bool,
}
#[derive(Clone, Default, PartialEq, Eq)]
struct RenderStyle {
    bold: bool,
    italic: bool,
    foreground: Option<String>,
    background: Option<String>,
}
#[derive(Clone)]
enum Event {
    Start(bool),
    End(bool, bool),
}
#[derive(Clone)]
struct Chunk {
    text: String,
    style: RenderStyle,
    before: Vec<Event>,
    after: Vec<Event>,
}
type Rendered = Vec<Chunk>;
fn text(s: impl Into<String>, style: &RenderStyle) -> Rendered {
    let s = s.into();
    if s.is_empty() { vec![] } else { vec![Chunk { text: s, style: style.clone(), before: vec![], after: vec![] }] }
}
fn append(out: &mut Rendered, items: Rendered) {
    for c in items {
        if c.text.is_empty() {
            continue;
        }
        if let Some(last) = out.last_mut()
            && last.after.is_empty()
            && c.before.is_empty()
            && c.after.is_empty()
            && last.style == c.style
        {
            last.text.push_str(&c.text);
        } else {
            out.push(c);
        }
    }
}
fn cat(parts: impl IntoIterator<Item = Rendered>) -> Rendered {
    let mut out = vec![];
    for p in parts {
        append(&mut out, p);
    }
    out
}
fn plain(r: &Rendered) -> String {
    r.iter().map(|c| c.text.as_str()).collect()
}
fn length(r: &Rendered) -> usize {
    r.iter().map(|c| c.text.chars().count()).sum()
}
fn surround(r: Rendered, l: &str, h: &str, s: &RenderStyle) -> Rendered {
    cat([text(l, s), r, text(h, s)])
}
fn map_all(r: &Rendered, map: fn(&str) -> Option<&'static str>) -> Option<Rendered> {
    let mut out = vec![];
    for c in r {
        let mut mapped = String::new();
        for ch in c.text.chars() {
            mapped.push_str(map(&ch.to_string())?);
        }
        let mut chunk = c.clone();
        chunk.text = mapped;
        append(&mut out, vec![chunk]);
    }
    Some(out)
}
fn script(r: Rendered, sup: bool, group: bool, s: &RenderStyle) -> Rendered {
    if r.is_empty() {
        return r;
    }
    if let Some(mapped) = map_all(&r, if sup { superscript } else { subscript }) {
        return mapped;
    }
    surround(
        r,
        if sup {
            if group { "^(" } else { "^" }
        } else if group {
            "_("
        } else {
            "_"
        },
        if group { ")" } else { "" },
        s,
    )
}
fn combining(r: Rendered, mark: &str) -> Rendered {
    r.into_iter()
        .map(|mut c| {
            c.text = c.text.chars().map(|ch| if ch == ' ' { " ".into() } else { format!("{ch}{mark}") }).collect();
            c
        })
        .collect()
}
fn trim(mut r: Rendered) -> Rendered {
    while let Some(c) = r.first_mut() {
        c.text = c.text.trim_start().into();
        if !c.text.is_empty() {
            break;
        }
        r.remove(0);
    }
    while let Some(c) = r.last_mut() {
        c.text = c.text.trim_end().into();
        if !c.text.is_empty() {
            break;
        }
        r.pop();
    }
    r
}
fn transition(out: &mut String, from: &RenderStyle, to: &RenderStyle) {
    if from.bold && !to.bold {
        out.push_str("\x1b[22m");
    }
    if from.italic && !to.italic {
        out.push_str("\x1b[23m");
    }
    let fg = from.foreground != to.foreground;
    let bg = from.background != to.background;
    if fg && bg && to.background.is_none() {
        out.push_str("\x1b[49m");
        out.push_str(to.foreground.as_deref().unwrap_or("\x1b[39m"));
    } else {
        if fg {
            out.push_str(to.foreground.as_deref().unwrap_or("\x1b[39m"));
        }
        if bg {
            out.push_str(to.background.as_deref().unwrap_or("\x1b[49m"));
        }
    }
    if !from.bold && to.bold {
        out.push_str("\x1b[1m");
    }
    if !from.italic && to.italic {
        out.push_str("\x1b[3m");
    }
}
fn render_ansi(r: Rendered) -> String {
    let mut out = String::new();
    let mut active = RenderStyle::default();
    for c in r {
        transition(&mut out, &active, &c.style);
        active = c.style;
        for e in c.before {
            event(&mut out, e, &mut active);
        }
        out.push_str(&c.text);
        for e in c.after {
            event(&mut out, e, &mut active);
        }
    }
    transition(&mut out, &active, &RenderStyle::default());
    out
}
fn event(out: &mut String, e: Event, active: &mut RenderStyle) {
    let (bold, start, restore) = match e {
        Event::Start(b) => (b, true, true),
        Event::End(b, r) => (b, false, r),
    };
    let value = if bold { &mut active.bold } else { &mut active.italic };
    if start {
        out.push_str(if bold { "\x1b[1m" } else { "\x1b[3m" });
    } else {
        if *value {
            out.push_str(if bold { "\x1b[22m" } else { "\x1b[23m" });
        }
        if restore {
            out.push_str(if bold { "\x1b[1m" } else { "\x1b[3m" });
        }
    }
    *value = restore;
}
fn scope(mut r: Rendered, bold: bool, parent: bool) -> Rendered {
    if !parent || r.is_empty() {
        return r;
    }
    let first = r.first_mut().unwrap();
    if if bold { first.style.bold } else { first.style.italic } {
        first.before.push(Event::Start(bold));
    }
    let last = r.last_mut().unwrap();
    if if bold { last.style.bold } else { last.style.italic } {
        last.after.push(Event::End(bold, true));
    }
    r
}
fn style_char(ch: char, s: &ParseStyle) -> String {
    let Some(font) = s.font else {
        return ch.to_string();
    };
    if let Some(hole) = alpha_holes(&format!("{font}:{ch}")) {
        return hole.into();
    }
    let (upper, lower, digit) = match font {
        "bold" => (0x1d400, 0x1d41a, Some(0x1d7ce)),
        "italic" => (0x1d434, 0x1d44e, None),
        "bolditalic" => (0x1d468, 0x1d482, None),
        "script" => (0x1d49c, 0x1d4b6, None),
        "boldscript" => (0x1d4d0, 0x1d4ea, None),
        "fraktur" => (0x1d504, 0x1d51e, None),
        "doublestruck" => (0x1d538, 0x1d552, Some(0x1d7d8)),
        "boldfraktur" => (0x1d56c, 0x1d586, None),
        "sans" => (0x1d5a0, 0x1d5ba, Some(0x1d7e2)),
        "sansbold" => (0x1d5d4, 0x1d5ee, Some(0x1d7ec)),
        "sansitalic" => (0x1d608, 0x1d622, None),
        "sansbolditalic" => (0x1d63c, 0x1d656, None),
        "mono" => (0x1d670, 0x1d68a, Some(0x1d7f6)),
        _ => return ch.to_string(),
    };
    let cp = if ch.is_ascii_uppercase() {
        Some(upper + ch as u32 - 65)
    } else if ch.is_ascii_lowercase() {
        Some(lower + ch as u32 - 97)
    } else if ch.is_ascii_digit() {
        digit.map(|d| d + ch as u32 - 48)
    } else {
        None
    };
    cp.and_then(char::from_u32).unwrap_or(ch).to_string()
}
pub fn is_math_font_command(name: &str) -> bool {
    fonts(name).is_some()
        || matches!(name, "textbf" | "textit" | "textsl" | "emph" | "textmd" | "textup" | "texttt" | "textsf")
}
#[derive(Clone)]
struct Argument {
    text: Rendered,
    group: bool,
}
struct LatexParser {
    source: Vec<char>,
    i: usize,
    foreground: Option<String>,
    background: Option<String>,
    true_color: bool,
}
impl LatexParser {
    fn new(s: &str, true_color: bool) -> Self {
        Self { source: s.chars().collect(), i: 0, foreground: None, background: None, true_color }
    }
    fn peek(&self) -> Option<char> {
        self.source.get(self.i).copied()
    }
    fn starts(&self, t: &str) -> bool {
        self.source[self.i..].iter().copied().zip(t.chars()).count() == t.chars().count()
            && self.source[self.i..].iter().take(t.chars().count()).copied().eq(t.chars())
    }
    fn current(&self, s: &ParseStyle) -> RenderStyle {
        RenderStyle {
            bold: s.bold,
            italic: s.italic,
            foreground: self.foreground.clone(),
            background: self.background.clone(),
        }
    }
    fn parse(&mut self, s: &ParseStyle, stop: bool) -> Rendered {
        let mut out = vec![];
        while let Some(c) = self.peek() {
            if c == '}' {
                if stop {
                    break;
                }
                self.i += 1;
                continue;
            }
            append(&mut out, self.node(s));
        }
        out
    }
    fn node(&mut self, s: &ParseStyle) -> Rendered {
        let c = self.peek().unwrap();
        let current = self.current(s);
        match c {
            '\\' => self.command(s),
            '{' => {
                self.i += 1;
                let fg = self.foreground.clone();
                let bg = self.background.clone();
                let r = self.parse(s, true);
                if self.peek() == Some('}') {
                    self.i += 1;
                }
                self.foreground = fg;
                self.background = bg;
                r
            }
            '^' | '_' => {
                self.i += 1;
                let a = self.argument(s);
                script(a.text, c == '^', a.group, &current)
            }
            '$' => {
                self.i += 1;
                vec![]
            }
            '~' => {
                self.i += 1;
                text(" ", &current)
            }
            '&' => {
                self.i += 1;
                text("  ", &current)
            }
            '\'' => {
                let mut n = 0;
                while self.peek() == Some('\'') {
                    n += 1;
                    self.i += 1;
                }
                text(
                    match n {
                        1 => "′".into(),
                        2 => "″".into(),
                        3 => "‴".into(),
                        4 => "⁗".into(),
                        _ => "′".repeat(n),
                    },
                    &current,
                )
            }
            '%' => {
                while let Some(c) = self.peek() {
                    self.i += 1;
                    if c == '\n' {
                        break;
                    }
                }
                vec![]
            }
            _ => {
                self.i += 1;
                text(style_char(c, s), &current)
            }
        }
    }
    fn command(&mut self, s: &ParseStyle) -> Rendered {
        self.i += 1;
        let Some(c) = self.peek() else {
            return vec![];
        };
        let current = self.current(s);
        if !c.is_ascii_alphabetic() {
            self.i += 1;
            return match c {
                '\\' => text("\n", &current),
                ',' | ':' | ';' | '>' => text(" ", &current),
                '!' | '/' | '(' | ')' | '[' | ']' => vec![],
                '|' => text("‖", &current),
                _ => text(c.to_string(), &current),
            };
        }
        let name = self.name();
        if self.peek() == Some('*') {
            self.i += 1;
        }
        self.apply(&name, s)
    }
    fn name(&mut self) -> String {
        let start = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
            self.i += 1;
        }
        self.source[start..self.i].iter().collect()
    }
    fn argument(&mut self, s: &ParseStyle) -> Argument {
        self.skip_spaces();
        let Some(c) = self.peek() else {
            return Argument { text: vec![], group: false };
        };
        let r = if c == '{' {
            self.i += 1;
            let r = self.parse(s, true);
            if self.peek() == Some('}') {
                self.i += 1;
            }
            return Argument { text: r, group: true };
        } else if c == '\\' {
            self.command(s)
        } else if c == '^' || c == '_' {
            self.i += 1;
            let a = self.argument(s);
            script(a.text, c == '^', a.group, &self.current(s))
        } else {
            self.i += 1;
            text(style_char(c, s), &self.current(s))
        };
        Argument { text: r, group: false }
    }
    fn skip_spaces(&mut self) {
        while self.peek() == Some(' ') {
            self.i += 1;
        }
    }
    fn raw_argument(&mut self) -> String {
        self.skip_spaces();
        let Some(c) = self.peek() else {
            return String::new();
        };
        if c != '{' {
            self.i += 1;
            if c != '\\' {
                return c.to_string();
            }
            let mut t = "\\".to_owned();
            if self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
                t.push_str(&self.name());
            } else if let Some(c) = self.peek() {
                t.push(c);
                self.i += 1;
            }
            return t;
        }
        self.i += 1;
        let mut depth = 1;
        let mut out = String::new();
        while let Some(c) = self.peek() {
            self.i += 1;
            if c == '\\' {
                out.push(c);
                if let Some(next) = self.peek() {
                    out.push(next);
                    self.i += 1;
                }
                continue;
            }
            if c == '{' {
                depth += 1;
            } else if c == '}' {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            out.push(c);
        }
        out
    }
    fn optional_raw(&mut self) -> Option<String> {
        self.skip_spaces();
        if self.peek() != Some('[') {
            return None;
        }
        self.i += 1;
        let mut brackets = 1;
        let mut braces = 0;
        let mut out = String::new();
        while let Some(c) = self.peek() {
            self.i += 1;
            if c == '\\' {
                out.push(c);
                if let Some(n) = self.peek() {
                    out.push(n);
                    self.i += 1;
                }
                continue;
            }
            if c == '{' {
                braces += 1;
            } else if c == '}' && braces > 0 {
                braces -= 1;
            } else if braces == 0 && c == '[' {
                brackets += 1;
            } else if braces == 0 && c == ']' {
                brackets -= 1;
                if brackets == 0 {
                    break;
                }
            }
            out.push(c);
        }
        Some(out)
    }
    fn optional(&mut self, s: &ParseStyle) -> Option<Argument> {
        self.optional_raw()
            .map(|raw| Argument { text: LatexParser::new(&raw, self.true_color).parse(s, false), group: true })
    }
    fn separator(&self) -> &'static str {
        if self.peek().is_some_and(|c| c.is_ascii_alphanumeric() || c == '\\') { " " } else { "" }
    }
    fn fraction(&self, n: Argument, d: Argument, s: &RenderStyle) -> Rendered {
        let first = n.text.first().map(|c| &c.style).unwrap_or(s);
        let same = n.text.iter().chain(&d.text).all(|c| &c.style == first);
        if same && let Some(v) = vulgar(&format!("{}/{}", plain(&n.text), plain(&d.text))) {
            return text(v, first);
        }
        let wrap = |a: Argument| if a.group && length(&a.text) > 1 { surround(a.text, "(", ")", s) } else { a.text };
        cat([wrap(n), text("/", s), wrap(d)])
    }
    fn read_color(&mut self) -> Option<(String, String)> {
        let model = self.optional_raw();
        let spec = self.raw_argument();
        latex_color(model.as_deref(), &spec, self.true_color)
    }
    fn delimiter(&mut self, s: &ParseStyle) -> Rendered {
        self.skip_spaces();
        let Some(c) = self.peek() else {
            return vec![];
        };
        self.i += 1;
        let current = self.current(s);
        if c == '.' {
            return vec![];
        }
        if c != '\\' {
            return text(style_char(c, s), &current);
        }
        let Some(d) = self.peek() else {
            return vec![];
        };
        if !d.is_ascii_alphabetic() {
            self.i += 1;
            return match d {
                '.' => vec![],
                '|' => text("‖", &current),
                _ => text(d.to_string(), &current),
            };
        }
        let name = self.name();
        text(symbols(&name).unwrap_or(&name), &current)
    }
    fn environment(&mut self, s: &ParseStyle) -> Rendered {
        let env = self.raw_argument().trim().to_owned();
        if matches!(
            env.as_str(),
            "array"
                | "tabular"
                | "array*"
                | "tabular*"
                | "alignedat"
                | "alignedat*"
                | "alignat"
                | "alignat*"
                | "gatheredat"
        ) {
            self.optional_raw();
            if self.peek() == Some('{') {
                self.raw_argument();
            }
        }
        let mut r = vec![];
        while self.peek().is_some() {
            if self.starts("\\end") {
                self.i += 4;
                self.raw_argument();
                break;
            }
            append(&mut r, self.node(s));
        }
        let mut r = trim(r);
        if matches!(env.as_str(), "cases" | "cases*" | "dcases" | "dcases*" | "rcases" | "drcases") {
            let breaks = regex::Regex::new(r"[ \t]*\n+[ \t]*").unwrap();
            let spaces = regex::Regex::new(r" {3,}").unwrap();
            for c in &mut r {
                c.text = spaces.replace_all(&breaks.replace_all(&c.text, "; "), "  ").into_owned();
            }
        }
        let (l, h) = match env.as_str() {
            "pmatrix" => ("(", ")"),
            "bmatrix" => ("[", "]"),
            "Bmatrix" => ("{", "}"),
            "vmatrix" => ("|", "|"),
            "Vmatrix" => ("‖", "‖"),
            "cases" | "cases*" | "dcases" | "dcases*" => ("{", ""),
            "rcases" | "drcases" => ("", "}"),
            _ => ("", ""),
        };
        surround(r, l, h, &self.current(s))
    }
    fn apply(&mut self, name: &str, s: &ParseStyle) -> Rendered {
        let current = self.current(s);
        if let Some(font) = fonts(name) {
            let mut next = s.clone();
            next.font = Some(font);
            return self.argument(&next).text;
        }
        if matches!(name, "textbf" | "textit" | "textsl" | "emph" | "textmd" | "textup" | "texttt" | "textsf") {
            let mut next = s.clone();
            let attribute = match name {
                "textbf" => {
                    next.bold = true;
                    Some(true)
                }
                "textit" | "textsl" | "emph" => {
                    next.italic = true;
                    Some(false)
                }
                "textmd" => {
                    next.bold = false;
                    None
                }
                "textup" => {
                    next.italic = false;
                    None
                }
                _ => None,
            };
            let r = self.argument(&next).text;
            return match attribute {
                Some(b) => scope(r, b, if b { s.bold } else { s.italic }),
                None => r,
            };
        }
        if text_commands(name).is_some() {
            return text(unescape_text(&self.raw_argument()), &current);
        }
        if name == "operatorname" {
            let raw = unescape_text(&self.raw_argument());
            return text(format!("{raw}{}", self.separator()), &current);
        }
        if let Some(accent) = accents(name) {
            return combining(self.argument(s).text, accent);
        }
        if matches!(name, "frac" | "dfrac" | "tfrac" | "cfrac") {
            let n = self.argument(s);
            let d = self.argument(s);
            return self.fraction(n, d, &current);
        }
        if name == "genfrac" {
            let l = self.argument(s).text;
            let h = self.argument(s).text;
            self.raw_argument();
            self.raw_argument();
            let n = self.argument(s);
            let d = self.argument(s);
            return cat([l, self.fraction(n, d, &current), h]);
        }
        if matches!(name, "binom" | "dbinom" | "tbinom") {
            let n = self.argument(s).text;
            let d = self.argument(s).text;
            return cat([text("C(", &current), n, text(", ", &current), d, text(")", &current)]);
        }
        if name == "sqrt" {
            let radical = if let Some(i) = self.optional(s) {
                match plain(&i.text).as_str() {
                    "2" => text("√", &current),
                    "3" => text("∛", &current),
                    "4" => text("∜", &current),
                    _ => cat([script(i.text, true, true, &current), text("√", &current)]),
                }
            } else {
                text("√", &current)
            };
            let r = self.argument(s).text;
            return cat([radical, if length(&r) > 1 { surround(r, "(", ")", &current) } else { r }]);
        }
        if name == "not" {
            let r = self.argument(s).text;
            return if let Some(v) = not_map(&plain(&r)) {
                text(v, r.first().map(|c| &c.style).unwrap_or(&current))
            } else {
                combining(r, "\u{0338}")
            };
        }
        if matches!(name, "overset" | "stackrel" | "underset") {
            let a = self.argument(s);
            let base = self.argument(s).text;
            return cat([base, script(a.text, name != "underset", true, &current)]);
        }
        if name == "prescript" {
            let a = self.argument(s).text;
            let b = self.argument(s).text;
            let base = self.argument(s).text;
            return cat([script(a, true, true, &current), script(b, false, true, &current), base]);
        }
        if let Some(arrow) = extensible_arrows(name) {
            let below = self.optional(s);
            let above = self.argument(s).text;
            return cat([
                text(arrow, &current),
                script(above, true, true, &current),
                below.map(|a| script(a.text, false, true, &current)).unwrap_or_default(),
            ]);
        }
        let surround_pair = match name {
            "boxed" | "fbox" => Some(("[", "]")),
            "overbrace" => Some(("⏞(", ")")),
            "underbrace" => Some(("⏟(", ")")),
            "overbracket" => Some(("⎴(", ")")),
            "underbracket" => Some(("⎵(", ")")),
            "overparen" => Some(("⏜(", ")")),
            "underparen" => Some(("⏝(", ")")),
            "pmod" => Some(("(mod ", ")")),
            "pod" | "tag" => Some(("(", ")")),
            _ => None,
        };
        if let Some((l, h)) = surround_pair {
            return surround(self.argument(s).text, l, h, &current);
        }
        if matches!(name, "cancel" | "bcancel" | "xcancel" | "sout") {
            let r = self.argument(s).text;
            return match name {
                "bcancel" => combining(r, "\u{20e5}"),
                "xcancel" => combining(combining(r, "\u{0338}"), "\u{20e5}"),
                "sout" => combining(r, "\u{0336}"),
                _ => combining(r, "\u{0338}"),
            };
        }
        if name == "substack" {
            let mut r = self.argument(s).text;
            let re = regex::Regex::new(r"\n+").unwrap();
            for c in &mut r {
                c.text = re.replace_all(&c.text, ",").into_owned();
            }
            return r;
        }
        if matches!(name, "left" | "right" | "middle")
            || regex::Regex::new(r"^(?:[bB]igg?|[bB]igg?[lrm])$").unwrap().is_match(name)
        {
            return self.delimiter(s);
        }
        match name {
            "begin" => return self.environment(s),
            "end" | "label" => {
                self.raw_argument();
                return vec![];
            }
            "bmod" => return text(" mod ", &current),
            "ref" | "eqref" => return text(format!("({})", unescape_text(&self.raw_argument())), &current),
            "url" => return text(unescape_text(&self.raw_argument()), &current),
            "href" => {
                self.raw_argument();
                return self.argument(s).text;
            }
            "textcolor" | "colorbox" => {
                let color = self.read_color();
                let fg = self.foreground.clone();
                let bg = self.background.clone();
                if let Some((f, b)) = color {
                    if name == "textcolor" {
                        self.foreground = Some(f);
                    } else {
                        self.background = Some(b);
                    }
                }
                let r = self.argument(s).text;
                self.foreground = fg;
                self.background = bg;
                return r;
            }
            "color" => {
                if let Some((f, _)) = self.read_color() {
                    self.foreground = Some(f);
                }
                return vec![];
            }
            "normalcolor" => {
                self.foreground = None;
                return vec![];
            }
            "fcolorbox" => {
                let fm = self.optional_raw();
                let fs = self.raw_argument();
                let frame = latex_color(fm.as_deref(), &fs, self.true_color);
                let bm = self.optional_raw().or(fm);
                let bs = self.raw_argument();
                let bg = latex_color(bm.as_deref(), &bs, self.true_color);
                let old = self.background.clone();
                if let Some((_, b)) = bg {
                    self.background = Some(b);
                }
                let body = self.argument(s).text;
                self.background = old;
                let mut frame_style = self.current(s);
                if let Some((f, _)) = frame {
                    frame_style.foreground = Some(f);
                }
                return cat([text("[", &frame_style), body, text("]", &frame_style)]);
            }
            "phantom" | "hphantom" => {
                let r = self.argument(s).text;
                return text(" ".repeat(length(&r)), &current);
            }
            "vphantom" => {
                self.argument(s);
                return vec![];
            }
            _ => {}
        }
        if functions(name).is_some() {
            return text(format!("{name}{}", self.separator()), &current);
        }
        if let Some(sym) = symbols(name) {
            return text(sym, &current);
        }
        match name {
            "displaystyle" | "textstyle" | "scriptstyle" | "scriptscriptstyle" | "limits" | "nolimits" | "nonumber"
            | "notag" | "negthinspace" | "negmedspace" | "negthickspace" => vec![],
            "quad" => text("  ", &current),
            "qquad" => text("    ", &current),
            "thinspace" | "enspace" | "medspace" | "thickspace" | "space" => text(" ", &current),
            _ => text(name, &current),
        }
    }
}
fn unescape_text(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\'
            && chars.peek().is_some_and(|c| matches!(c, '&' | '%' | '$' | '#' | '_' | '{' | '}') || c.is_whitespace())
        {
            out.push(chars.next().unwrap());
        } else {
            out.push(if c == '~' { ' ' } else { c });
        }
    }
    out
}
pub fn latex_to_unicode(source: &str) -> String {
    latex_to_unicode_with_color(source, true)
}
pub fn latex_to_unicode_with_color(source: &str, true_color: bool) -> String {
    render_ansi(LatexParser::new(source, true_color).parse(&ParseStyle::default(), false))
}

fn number(raw: &str) -> Option<f64> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let n = if let Some(p) = s.strip_suffix('%') {
        number(p)? / 100.
    } else if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(h, 16).ok()? as f64
    } else if let Some(b) = s.strip_prefix("0b").or_else(|| s.strip_prefix("0B")) {
        u64::from_str_radix(b, 2).ok()? as f64
    } else if let Some(o) = s.strip_prefix("0o").or_else(|| s.strip_prefix("0O")) {
        u64::from_str_radix(o, 8).ok()? as f64
    } else {
        s.parse().ok()?
    };
    n.is_finite().then_some(n)
}
fn components(s: &str, n: usize) -> Option<Vec<f64>> {
    let values: Vec<_> = s
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .map(number)
        .collect::<Option<_>>()?;
    (values.len() == n).then_some(values)
}
fn css_color(spec: &str) -> Option<[f64; 3]> {
    let spec = spec.trim();
    if spec.contains('!') {
        let mut parts = spec.split('!');
        let mut current = css_color(parts.next()?)?;
        while let Some(p) = parts.next() {
            let t = (number(p)? / 100.).clamp(0., 1.);
            let next = css_color(parts.next().unwrap_or("white"))?;
            for i in 0..3 {
                current[i] = current[i] * t + next[i] * (1. - t);
            }
        }
        return Some(current);
    }
    let lower = spec.to_lowercase();
    let spec = latex_named_colors(&lower).unwrap_or(spec);
    css::parse_rgb(spec)
}
fn rgb_model(model: Option<&str>, spec: &str) -> Option<[f64; 3]> {
    let Some(model) = model.map(str::trim) else {
        return css_color(spec);
    };
    if model.is_empty() || model == "named" {
        return css_color(spec);
    }
    let lower = model.to_lowercase();
    match lower.as_str() {
        "html" => css_color(&format!("#{}", spec.trim().trim_start_matches('#'))),
        "rgb" => {
            let v = components(spec, 3)?;
            Some([0, 1, 2].map(|i| if model == "RGB" { v[i] } else { v[i].clamp(0., 1.) * 255. }))
        }
        "cmyk" => {
            let v = components(spec, 4)?;
            Some([0, 1, 2].map(|i| 255. * (1. - v[i].clamp(0., 1.)) * (1. - v[3].clamp(0., 1.))))
        }
        "gray" | "grey" => {
            let v = components(spec, 1)?[0];
            let b = (if model == "Gray" || model == "Grey" { v / 15. } else { v }).clamp(0., 1.) * 255.;
            Some([b; 3])
        }
        "hsb" | "hsv" => {
            let v = components(spec, 3)?;
            let h = (v[0] * if model == "Hsb" || model == "HSV" { 1. } else { 360. }) % 360. / 60.;
            let sat = v[1].clamp(0., 1.);
            let value = v[2].clamp(0., 1.);
            let c = value * sat;
            let x = c * (1. - (h % 2. - 1.).abs());
            let m = value - c;
            let base = if h < 1. {
                [c, x, 0.]
            } else if h < 2. {
                [x, c, 0.]
            } else if h < 3. {
                [0., c, x]
            } else if h < 4. {
                [0., x, c]
            } else if h < 5. {
                [x, 0., c]
            } else {
                [c, 0., x]
            };
            Some(base.map(|n| (n + m) * 255.))
        }
        "wave" => {
            let w = number(spec)?;
            if !(380. ..=780.).contains(&w) {
                return None;
            }
            let base = if w < 440. {
                [-(w - 440.) / 60., 0., 1.]
            } else if w < 490. {
                [0., (w - 440.) / 50., 1.]
            } else if w < 510. {
                [0., 1., -(w - 510.) / 20.]
            } else if w < 580. {
                [(w - 510.) / 70., 1., 0.]
            } else if w < 645. {
                [1., -(w - 645.) / 65., 0.]
            } else {
                [1., 0., 0.]
            };
            let f = if w < 420. {
                0.3 + 0.7 * (w - 380.) / 40.
            } else if w > 700. {
                0.3 + 0.7 * (780. - w) / 80.
            } else {
                1.
            };
            Some(base.map(|n| n * f * 255.))
        }
        _ => css_color(spec),
    }
}
pub fn latex_color(model: Option<&str>, spec: &str, true_color: bool) -> Option<(String, String)> {
    let s = unescape_text(spec);
    let rgb = rgb_model(model, s.trim())?.map(|n| (n.clamp(0., 255.) + 0.5).floor() as u8);
    let suffix = if true_color {
        format!("2;{};{};{}m", rgb[0], rgb[1], rgb[2])
    } else {
        let index = css::ansi256(rgb);
        format!("5;{index}m")
    };
    Some((format!("\x1b[38;{suffix}"), format!("\x1b[48;{suffix}")))
}

#[cfg(test)]
#[test]
fn independent_bun_css_oracle_group() {
    let cases: serde_json::Value = serde_json::from_str(include_str!("markdown_css_color_oracle.json")).unwrap();
    for case in cases.as_array().unwrap() {
        let input = case["input"].as_str().unwrap();
        for (true_color, expected) in [(true, "true_color"), (false, "ansi256")] {
            let actual = latex_color(None, input, true_color);
            assert_eq!(
                actual.as_ref().map(|v| v.0.as_str()),
                case[expected].as_str(),
                "independent Bun 1.4.0: {input}, trueColor={true_color}"
            );
            if let Some((foreground, background)) = actual {
                assert_eq!(background, foreground.replacen("\x1b[38;", "\x1b[48;", 1), "background mapping: {input}");
            }
        }
    }
}
pub fn is_bare_math_environment(env: &str) -> bool {
    matches!(
        env.trim_end_matches('*'),
        "matrix"
            | "smallmatrix"
            | "pmatrix"
            | "bmatrix"
            | "Bmatrix"
            | "vmatrix"
            | "Vmatrix"
            | "cases"
            | "dcases"
            | "rcases"
            | "drcases"
            | "aligned"
            | "alignedat"
            | "align"
            | "alignat"
            | "split"
            | "gathered"
            | "gatheredat"
            | "gather"
            | "multline"
            | "equation"
            | "eqnarray"
            | "array"
            | "subarray"
    )
}

fn bare_command(source: &str) -> bool {
    regex::Regex::new(r"\\(?:operatorname|frac|dfrac|tfrac|cfrac|genfrac|sqrt|sum|prod|coprod|int|iint|iiint|lim|alpha|beta|gamma|delta|epsilon|varepsilon|theta|lambda|mu|sigma|phi|varphi|pi|omega|infty|partial|nabla|forall|exists|mathbb|mathcal|mathscr|mathbf|mathrm|left|right|begin|phantom|hphantom|vphantom|cdots|ldots|dots|to|rightarrow|leftarrow|leq|geq|neq|times|cdot|overline|underline|vec|hat|bar|textbf|textit|textsl|emph|textmd|textup|texttt|textsf|textcolor|color|normalcolor|colorbox|fcolorbox)\b").unwrap().is_match(source)
}
fn bare_lines(source: &str) -> String {
    let env = regex::Regex::new(r"\\(?:begin|end)\{([^}]*)\}").unwrap();
    source
        .split('\n')
        .map(|line| {
            let trimmed = line.trim();
            let shielded = env.captures(trimmed).is_some_and(|caps| !is_bare_math_environment(&caps[1]));
            if trimmed.is_empty()
                || !trimmed.contains('\\')
                || shielded
                || !bare_command(trimmed)
                || !(trimmed.starts_with('\\')
                    || trimmed.chars().any(|c| matches!(c, '=' | '<' | '>' | '^' | '_' | '{' | '}' | '&')))
            {
                line.to_owned()
            } else {
                regex::Regex::new(r"\n+").unwrap().replace_all(&latex_to_unicode(line), " ").into_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
fn render_bare_math(source: &str) -> String {
    let mut out = String::new();
    let mut i = 0;
    loop {
        let Some(begin) = source[i..].find("\\begin{").map(|n| i + n) else {
            out.push_str(&bare_lines(&source[i..]));
            return out;
        };
        let env_start = begin + "\\begin{".len();
        let Some(env_end) = source[env_start..].find('}').map(|n| env_start + n) else {
            out.push_str(&bare_lines(&source[i..]));
            return out;
        };
        let env = &source[env_start..env_end];
        let close_token = format!("\\end{{{env}}}");
        let Some(close) = source[env_end + 1..].find(&close_token).map(|n| env_end + 1 + n) else {
            out.push_str(&bare_lines(&source[i..env_end + 1]));
            i = env_end + 1;
            continue;
        };
        let block_end = close + close_token.len();
        if !is_bare_math_environment(env) {
            out.push_str(&bare_lines(&source[i..begin]));
            out.push_str(&source[begin..block_end]);
            i = block_end;
            continue;
        }
        let line_start = source[..begin].rfind('\n').map(|n| n + 1).unwrap_or(0);
        let prefix = &source[line_start..begin];
        let mut start = if prefix.contains('\\') || prefix.contains('=') { line_start } else { begin };
        if start == begin && prefix.trim().is_empty() && line_start > 0 {
            let previous_end = line_start - 1;
            let previous_start = source[..previous_end].rfind('\n').map(|n| n + 1).unwrap_or(0);
            if regex::Regex::new(r"[=(\[{]\s*$").unwrap().is_match(&source[previous_start..previous_end]) {
                start = previous_start;
            }
        }
        // The fixed scanner permits a preceding lhs; clamp to already-consumed text.
        start = start.max(i);
        out.push_str(&bare_lines(&source[i..start]));
        out.push_str(
            &regex::Regex::new(r"\n+").unwrap().replace_all(&latex_to_unicode(&source[start..block_end]), " "),
        );
        i = block_end;
    }
}
pub fn render_math_in_text(source: &str) -> String {
    if source.is_empty()
        || !(source.contains('$')
            || source.contains("\\(")
            || source.contains("\\[")
            || source.contains("\\begin")
            || bare_command(source))
    {
        return source.into();
    }
    let units: Vec<u16> = source.encode_utf16().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < units.len() {
        let c = units[i];
        if c == 92 {
            if units.get(i + 1) == Some(&92) {
                out.push_str("\\\\");
                i += 2;
                continue;
            }
            if units.get(i + 1) == Some(&36) {
                out.push('$');
                i += 2;
                continue;
            }
            if let Some(span) = math_span_at(source, i, i) {
                out.push_str(&regex::Regex::new(r"\n+").unwrap().replace_all(&latex_to_unicode(&span.body), " "));
                i = span.end;
                continue;
            }
        } else if c == 36 {
            if let Some(span) = math_span_at(source, i, i) {
                out.push_str(&regex::Regex::new(r"\n+").unwrap().replace_all(&latex_to_unicode(&span.body), " "));
                i = span.end;
                continue;
            }
            if units.get(i + 1) == Some(&36) {
                out.push_str("$$");
                i += 2;
                continue;
            }
        }
        let count = if (0xd800..=0xdbff).contains(&c) && units.get(i + 1).is_some_and(|u| (0xdc00..=0xdfff).contains(u))
        {
            2
        } else {
            1
        };
        out.push_str(&String::from_utf16_lossy(&units[i..i + count]));
        i += count;
    }
    render_bare_math(&out)
}

#[cfg(test)]
#[path = "markdown_math_original_tests.rs"]
mod original_tests;
