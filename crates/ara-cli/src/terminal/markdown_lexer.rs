//! Fixed OMP static renderer adapter over pulldown-cmark 0.13.0 CommonMark/GFM.
//! This is not the public Marked token/extension API or its streaming lexer.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::markdown_math::{is_bare_math_environment, math_span_at};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag};
use std::ops::Range;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Paragraph,
    Heading(u8),
    Quote,
    CodeBlock(String),
    List(Option<u64>),
    Item,
    Table,
    TableHead,
    TableRow,
    TableCell,
    Strong,
    Em,
    Del,
    Link(String),
    Image(String),
    HtmlBlock,
    Html,
    Text,
    Code,
    Br,
    SoftBreak,
    Hr,
    Space,
    Definition,
    Checkbox(bool),
    Math { display: bool },
}
#[derive(Clone, Debug)]
pub struct Token {
    pub kind: Kind,
    pub text: String,
    pub raw: String,
    pub children: Vec<Token>,
    pub start: usize,
    pub end: usize,
}
struct Prepared {
    text: String,
    map: Vec<usize>,
    math: Vec<(String, bool)>,
    marker_start: String,
    nul_marker: String,
}
impl Prepared {
    fn original(&self, range: &Range<usize>) -> Range<usize> {
        self.map.get(range.start).copied().unwrap_or(0)
            ..self.map.get(range.end).copied().unwrap_or_else(|| *self.map.last().unwrap_or(&0))
    }
}
fn options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH
}
fn prepare(source: &str) -> Prepared {
    // Protect actual code constructs before applying OMP math grammar. No parser
    // math option is enabled: its currency/backslash semantics are different.
    let mut protected = vec![];
    let mut code_start = None;
    for (event, range) in Parser::new_ext(source, options()).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(_)) => code_start = Some(range.start),
            Event::End(pulldown_cmark::TagEnd::CodeBlock) => {
                if let Some(start) = code_start.take() {
                    protected.push(start..range.end);
                }
            }
            Event::Code(_) => protected.push(range),
            _ => {}
        }
    }
    protected.sort_by_key(|r| r.start);
    let mut marker_start = "\u{e000}ARA-MATH-0:".to_owned();
    let mut nonce = 0;
    while source.contains(&marker_start) {
        nonce += 1;
        marker_start = format!("\u{e000}ARA-MATH-{nonce}:");
    }
    let mut nul_marker = "\u{e000}ARA-NUL-0\u{e001}".to_owned();
    let mut nul_nonce = 0;
    while source.contains(&nul_marker) {
        nul_nonce += 1;
        nul_marker = format!("\u{e000}ARA-NUL-{nul_nonce}\u{e001}");
    }
    let mut p = Prepared { text: String::new(), map: vec![], math: vec![], marker_start, nul_marker };
    let mut byte_to_unit = vec![0; source.len() + 1];
    let mut unit_to_byte = vec![];
    let mut unit = 0;
    for (byte, ch) in source.char_indices() {
        byte_to_unit[byte..byte + ch.len_utf8()].fill(unit);
        unit_to_byte.extend(std::iter::repeat_n(byte, ch.len_utf16()));
        unit += ch.len_utf16();
    }
    byte_to_unit[source.len()] = unit;
    unit_to_byte.push(source.len());
    let mut i = 0;
    while i < source.len() {
        if let Some(range) = protected.iter().find(|r| r.start <= i && i < r.end) {
            let end = range.end;
            copy_source(&mut p, source, i, end);
            i = end;
            continue;
        }
        if (i == 0 || source.as_bytes()[i - 1] == b'\n')
            && let Some(end) = custom_rule_at(source, i)
        {
            let replacement = "***";
            p.text.push_str(replacement);
            p.map.extend(std::iter::repeat_n(i, replacement.len()));
            i = end;
            continue;
        }
        let at = byte_to_unit[i];
        let candidate =
            source.as_bytes()[i] == b'$' || source[i..].starts_with("\\(") || source[i..].starts_with("\\[");
        let mut found = candidate.then(|| math_span_at(source, at, 0)).flatten().map(|span| {
            let end = unit_to_byte[span.end];
            (span.body, span.display, end)
        });
        if found.is_none() && source[i..].starts_with("\\begin{") {
            let env_start = i + 7;
            if let Some(close) = source[env_start..].find('}').map(|n| n + env_start) {
                let env = &source[env_start..close];
                let line_start = source[..i].rfind('\n').map(|n| n + 1).unwrap_or(0);
                let prefix = &source[line_start..i];
                if is_bare_math_environment(env) && prefix.trim().is_empty() {
                    let closer = format!("\\end{{{env}}}");
                    if let Some(end) = source[close + 1..].find(&closer).map(|n| close + 1 + n + closer.len()) {
                        found = Some((source[i..end].into(), true, end));
                    }
                }
            }
        }
        if let Some((body, display, end)) = found {
            if !protected.iter().any(|r| r.start < end && r.end > i) {
                let id = p.math.len();
                p.math.push((body, display));
                let marker = format!("{}{id}\u{e001}", p.marker_start);
                p.map.extend(std::iter::repeat_n(i, marker.len()));
                p.text.push_str(&marker);
                i = end;
                continue;
            }
        }
        let end = i + source[i..].chars().next().unwrap().len_utf8();
        copy_source(&mut p, source, i, end);
        i = end;
    }
    p.map.push(source.len());
    p
}
fn copy_source(p: &mut Prepared, source: &str, start: usize, end: usize) {
    for (offset, ch) in source[start..end].char_indices() {
        let at = start + offset;
        if ch == '\0' {
            // CommonMark substitutes NUL; fixed OMP keeps it as ordinary prose.
            p.text.push_str(&p.nul_marker);
            p.map.extend(std::iter::repeat_n(at, p.nul_marker.len()));
        } else {
            p.text.push(ch);
            p.map.extend(at..at + ch.len_utf8());
        }
    }
}
fn token(kind: Kind, text: String, raw: String, range: Range<usize>) -> Token {
    Token { kind, text, raw, children: vec![], start: range.start, end: range.end }
}
fn append_text(out: &mut Vec<Token>, value: &str, range: Range<usize>, p: &Prepared) {
    let decoded = value.replace(&p.nul_marker, "\0");
    let value = decoded.as_str();
    let mut i = 0;
    while let Some(marker) = value[i..].find(&p.marker_start).map(|n| i + n) {
        if marker > i {
            out.push(token(Kind::Text, value[i..marker].into(), value[i..marker].into(), range.clone()));
        }
        let start = marker + p.marker_start.len();
        let Some(end) = value[start..].find('\u{e001}').map(|n| start + n) else {
            break;
        };
        let Some((body, display)) = value[start..end].parse::<usize>().ok().and_then(|id| p.math.get(id)) else {
            out.push(token(
                Kind::Text,
                value[marker..end + '\u{e001}'.len_utf8()].into(),
                String::new(),
                range.clone(),
            ));
            i = end + '\u{e001}'.len_utf8();
            continue;
        };
        out.push(token(Kind::Math { display: *display }, body.clone(), String::new(), range.clone()));
        i = end + '\u{e001}'.len_utf8();
    }
    if i < value.len() {
        out.push(token(Kind::Text, value[i..].into(), value[i..].into(), range));
    }
}
pub fn lex_document(source: &str) -> Vec<Token> {
    let p = prepare(source);
    let mut roots = vec![];
    let mut stack: Vec<Token> = vec![];
    let parser = Parser::new_ext(&p.text, options());
    let definitions: Vec<_> = parser
        .reference_definitions()
        .iter()
        .map(|(_, definition)| {
            let mut range = p.original(&definition.span);
            while source.as_bytes().get(range.end).is_some_and(|b| matches!(b, b' ' | b'\t')) {
                range.end += 1;
            }
            if source.as_bytes().get(range.end) == Some(&b'\n') {
                range.end += 1;
            }
            range
        })
        .collect();
    for (event, range) in parser.into_offset_iter() {
        let original = p.original(&range);
        let raw = source.get(original.clone()).unwrap_or("").to_owned();
        match event {
            Event::Start(tag) => {
                let kind = match tag {
                    Tag::Paragraph => Kind::Paragraph,
                    Tag::Heading { level, .. } => Kind::Heading(level as u8),
                    Tag::BlockQuote(_) => Kind::Quote,
                    Tag::CodeBlock(code) => Kind::CodeBlock(match code {
                        CodeBlockKind::Fenced(s) => s.split_whitespace().next().unwrap_or("").into(),
                        CodeBlockKind::Indented => String::new(),
                    }),
                    Tag::List(start) => Kind::List(start),
                    Tag::Item => Kind::Item,
                    Tag::Table(_) => Kind::Table,
                    Tag::TableHead => Kind::TableHead,
                    Tag::TableRow => Kind::TableRow,
                    Tag::TableCell => Kind::TableCell,
                    Tag::Strong => Kind::Strong,
                    Tag::Emphasis => Kind::Em,
                    Tag::Strikethrough => Kind::Del,
                    Tag::Link { dest_url, .. } => Kind::Link(dest_url.into_string()),
                    Tag::Image { dest_url, .. } => Kind::Image(dest_url.into_string()),
                    Tag::HtmlBlock => Kind::HtmlBlock,
                    _ => Kind::Paragraph,
                };
                stack.push(token(kind, String::new(), raw, original));
            }
            Event::End(_) => {
                if let Some(mut node) = stack.pop() {
                    node.end = node.end.max(original.end);
                    node.raw = source.get(node.start..node.end).unwrap_or("").into();
                    if let Some(parent) = stack.last_mut() {
                        parent.children.push(node);
                    } else {
                        roots.push(node);
                    }
                }
            }
            event => {
                let out = if let Some(parent) = stack.last_mut() { &mut parent.children } else { &mut roots };
                match event {
                    Event::Text(s) => {
                        // pulldown synthesizes one Text event per decoded entity,
                        // with the original entity's exact range (firstpass.rs).
                        // Marked preserves it for OMP's smaller, one-pass terminal
                        // whitelist; keep raw here to avoid broad or double decode.
                        let entity_event = raw.starts_with('&')
                            && raw.ends_with(';')
                            && raw[1..raw.len() - 1].bytes().all(|b| b.is_ascii_alphanumeric() || b == b'#');
                        append_text(out, if entity_event { &raw } else { &s }, original, &p);
                    }
                    Event::Code(s) => out.push(token(Kind::Code, s.replace(&p.nul_marker, "\0"), raw, original)),
                    Event::Html(_) | Event::InlineHtml(_) => {
                        // HTML preserves original bytes; math markers must not leak.
                        out.push(token(Kind::Html, raw.clone(), raw, original))
                    }
                    Event::SoftBreak => out.push(token(Kind::SoftBreak, "\n".into(), raw, original)),
                    Event::HardBreak => out.push(token(Kind::Br, "\n".into(), raw, original)),
                    Event::Rule => out.push(token(Kind::Hr, String::new(), raw, original)),
                    Event::TaskListMarker(b) => out.push(token(Kind::Checkbox(b), String::new(), raw, original)),
                    _ => {}
                }
            }
        }
    }
    for range in definitions {
        if !roots.iter().any(|node| node.start <= range.start && node.end >= range.end) {
            roots.push(token(Kind::Definition, String::new(), source.get(range.clone()).unwrap_or("").into(), range));
        }
    }
    roots.sort_by_key(|node| node.start);
    // Fixed marked/core.ts HTML block owner consumes physical source lines
    // until a matching closing tag or EOF. CommonMark stops at a blank line;
    // for an unclosed opening tag the remaining source is one HTML body, not
    // subsequent Markdown roots. Keep already matching closed/comment scopes.
    let html_open = regex::Regex::new(r"^ {0,3}<([A-Za-z][A-Za-z0-9_-]*)").unwrap();
    let mut scoped = Vec::with_capacity(roots.len());
    let mut unclosed_html_start = None;
    for mut node in roots {
        if node.kind == Kind::HtmlBlock
            && node.end < source.len()
            && let Some(open) = html_open.captures(&source[node.start..])
        {
            let closing = regex::Regex::new(&format!("(?i)</{}>", regex::escape(&open[1]))).unwrap();
            if !closing.is_match(&source[node.start..]) {
                unclosed_html_start = Some(node.start);
                node.end = source.len();
                node.raw = source[node.start..].into();
                node.children = vec![token(Kind::Html, node.raw.clone(), node.raw.clone(), node.start..node.end)];
                scoped.push(node);
                break;
            }
        }
        scoped.push(node);
    }
    let mut roots = scoped;
    for node in &mut roots {
        if matches!(node.kind, Kind::List(_) | Kind::HtmlBlock) && unclosed_html_start != Some(node.start) {
            let trimmed = node.raw.trim_end_matches([' ', '\t', '\n']);
            let mut end = node.start + trimmed.len();
            if source.as_bytes().get(end) == Some(&b'\n') {
                end += 1;
            }
            node.end = end;
            node.raw = source[node.start..end].into();
        }
        if node.kind == Kind::Paragraph {
            for i in 1..node.children.len() {
                let (before, after) = node.children.split_at_mut(i);
                let previous = &before[i - 1];
                let current = &mut after[0];
                if previous.kind == Kind::SoftBreak && current.kind == Kind::Text && previous.end <= current.start {
                    let indent = &source[previous.end..current.start];
                    if !indent.is_empty() && indent.bytes().all(|b| matches!(b, b' ' | b'\t')) {
                        current.text.insert_str(0, indent);
                    }
                }
            }
        }
    }
    // pulldown omits blank tokens. The static spacing adapter owns these gaps;
    // preserve one source-backed Space exactly as the fixed renderer does.
    let mut out = vec![];
    let mut prior = 0;
    for node in roots {
        if node.start >= prior && source[prior..node.start].contains('\n') {
            out.push(token(Kind::Space, String::new(), source[prior..node.start].into(), prior..node.start));
        }
        prior = node.end;
        out.push(node);
    }
    if source[prior..].contains('\n') {
        out.push(token(Kind::Space, String::new(), source[prior..].into(), prior..source.len()));
    }
    merge_adjacent_text(&mut out);
    adapt_inline(&mut out, source, false);
    out
}
fn merge_adjacent_text(tokens: &mut Vec<Token>) {
    let mut merged: Vec<Token> = Vec::with_capacity(tokens.len());
    for mut token in std::mem::take(tokens) {
        merge_adjacent_text(&mut token.children);
        if token.kind == Kind::Text
            && let Some(previous) = merged.last_mut().filter(|t| t.kind == Kind::Text)
        {
            previous.text.push_str(&token.text);
            previous.raw.push_str(&token.raw);
            previous.end = token.end;
        } else {
            merged.push(token);
        }
    }
    *tokens = merged;
}
fn custom_rule_at(source: &str, start: usize) -> Option<usize> {
    let end = source[start..].find('\n').map(|n| start + n).unwrap_or(source.len());
    let line = &source[start..end];
    let leading = line.chars().take_while(|c| *c == ' ').count();
    if leading > 3 {
        return None;
    }
    let trim = line.trim_end_matches([' ', '\t']);
    let mut glyphs = trim[leading..].chars().filter(|c| !matches!(c, ' ' | '\t'));
    let first = glyphs.next()?;
    if !matches!(first, '─' | '━' | '═' | '=' | '–' | '—') {
        return None;
    }
    let tail = glyphs.collect::<Vec<_>>();
    if tail.len() < 2 || tail.iter().any(|c| *c != first) {
        return None;
    }
    if first == '=' && start > 0 {
        let previous_end = start - 1;
        let previous_start = source[..previous_end].rfind('\n').map(|n| n + 1).unwrap_or(0);
        if !source[previous_start..previous_end].trim().is_empty() {
            return None;
        }
    }
    Some(end)
}
fn adapt_inline(tokens: &mut Vec<Token>, source: &str, in_link: bool) {
    let mut out = vec![];
    for mut node in std::mem::take(tokens) {
        let nested_link = in_link || matches!(node.kind, Kind::Link(_) | Kind::Image(_) | Kind::CodeBlock(_));
        adapt_inline(&mut node.children, source, nested_link);
        if node.kind == Kind::Del {
            let strict = regress::Regex::new(r"^(~~)(?=[^\s~])((?:\\.|[^\\])*?(?:\\.|[^\s~\\]))\1(?=[^~]|$)").unwrap();
            if !strict.find(&node.raw).is_some_and(|m| m.range().end == node.raw.len()) {
                let count = node.raw.chars().take_while(|c| *c == '~').count();
                let prefix = "~".repeat(count);
                let mut children = vec![token(Kind::Text, prefix.clone(), prefix.clone(), node.start..node.start)];
                children.extend(node.children);
                children.push(token(Kind::Text, prefix.clone(), prefix, node.end..node.end));
                node.kind = Kind::Paragraph;
                node.children = children;
            }
        }
        if node.kind != Kind::Text || in_link {
            out.push(node);
            continue;
        }
        let value = node.text.clone();
        let re = regex::Regex::new(
            r"(?i)(?:(?:https?://|ftp://|www\.)[^\s<]+|[A-Za-z0-9._+-]+@[A-Za-z0-9_-]+(?:\.[A-Za-z0-9_-]+)+)",
        )
        .unwrap();
        let mut prior = 0;
        for m in re.find_iter(&value) {
            let is_email = m.as_str().contains('@') && !m.as_str().contains("://");
            let preceding = if m.start() > 0 {
                value[..m.start()].chars().next_back()
            } else {
                source[..node.start.min(source.len())].chars().next_back()
            };
            if !is_email && preceding.is_some_and(|c| !c.is_whitespace() && !matches!(c, '*' | '_' | '~' | '(')) {
                continue;
            }
            let candidate = m.as_str();
            let mut end = candidate.trim_end_matches(['.', ',', ':', ';', '!', '?']).len();
            let opens = candidate[..end].chars().filter(|c| *c == '(').count();
            let mut closes = candidate[..end].chars().filter(|c| *c == ')').count();
            while closes > opens && candidate[..end].ends_with(')') {
                end -= 1;
                closes -= 1;
            }
            if end == 0 {
                continue;
            }
            let text = &candidate[..end];
            if m.start() > prior {
                out.push(token(Kind::Text, value[prior..m.start()].into(), String::new(), node.start..node.end));
            }
            let href = if is_email {
                format!("mailto:{text}")
            } else if text.starts_with("www.") {
                format!("http://{text}")
            } else {
                text.into()
            };
            let mut link = token(Kind::Link(href), text.into(), text.into(), node.start..node.end);
            link.children.push(token(Kind::Text, text.into(), text.into(), node.start..node.end));
            out.push(link);
            prior = m.start() + end;
        }
        if prior == 0 {
            out.push(node);
        } else if prior < value.len() {
            out.push(token(Kind::Text, value[prior..].into(), String::new(), node.start..node.end));
        }
    }
    *tokens = out;
}
pub fn plain_inline(tokens: &[Token]) -> String {
    tokens.iter().map(|t| if t.children.is_empty() { t.text.clone() } else { plain_inline(&t.children) }).collect()
}
