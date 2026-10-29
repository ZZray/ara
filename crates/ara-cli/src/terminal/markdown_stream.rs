//! Fixed OMP 596f2da markdown.ts streaming state and source predicates.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
//! The existing MarkdownComponent owns rendering. This module owns metadata.
use super::markdown_lexer::{self, Kind, Token};
use regex::Regex;
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicU64, Ordering},
};

static FAST_TAIL_SPLICES: AtomicU64 = AtomicU64::new(0);
pub fn fast_tail_splices() -> u64 {
    FAST_TAIL_SPLICES.load(Ordering::Relaxed)
}
pub fn reset_fast_tail_splices() {
    FAST_TAIL_SPLICES.store(0, Ordering::Relaxed);
}
pub(super) fn record_actual_splice() {
    FAST_TAIL_SPLICES.fetch_add(1, Ordering::Relaxed);
}

struct Gates {
    reference: Regex,
    line_start: Regex,
    run_end: Regex,
    swatch: Regex,
    entity: Regex,
    url: Regex,
    url_prefix: Regex,
    literal: Regex,
    underscore: Regex,
    underscore_word: Regex,
    cmark_word: Regex,
    table: Regex,
}
fn gates() -> &'static Gates {
    static GATES: OnceLock<Gates> = OnceLock::new();
    GATES.get_or_init(|| Gates {
        reference: Regex::new(r"(?m)^ {0,3}\[(?:\\.|[^\]\\])+\]:").unwrap(),
        line_start: Regex::new(r"^ {0,3}(?:#{1,6}(?:[ \t]|$)|>|[0-9]{1,9}[.)](?:[ \t]|$)|[*+=–—─━═-](?:[ \t]|$)|(?:[*+=–—─━═-][ \t]*){2,}[ \t]*$)").unwrap(),
        run_end: Regex::new(r"(?:[ \t\\]|#[0-9a-fA-F]{3,8}|#+)$").unwrap(),
        swatch: Regex::new(r"#[0-9a-fA-F]{0,2}$").unwrap(),
        entity: Regex::new(r"&(?:[A-Za-z0-9#]{0,31}|#[0-9]{1,7};|#[xX][0-9a-fA-F]{1,6};)$").unwrap(),
        url: Regex::new(r"(?i)(?:https?|ftp)://|www\.[A-Za-z0-9]|[A-Za-z0-9._%+-]+@").unwrap(),
        url_prefix: Regex::new(r"(?:https?|ftp):?/{0,2}$|www\.$|[A-Za-z0-9._+-]+@[A-Za-z0-9._+-]*$").unwrap(),
        literal: Regex::new(r"[*~`\[\]<>()$&#]").unwrap(),
        underscore: Regex::new(r"(?:^|[^A-Za-z0-9_])_").unwrap(),
        underscore_word: Regex::new(r"[A-Za-z0-9_\p{L}\p{N}]$").unwrap(),
        cmark_word: Regex::new(r"^[^\s\p{P}\p{S}]").unwrap(),
        table: Regex::new(r"^\s*(?:\|[\s:]*-+\s*(?:\|[\s:]*-+\s*)*|[\s:]*-+\s*(?:\|[\s:]*-+\s*)+)\|?\s*$").unwrap(),
    })
}
pub fn fast_line_start_hazard(line: &str) -> bool {
    gates().line_start.is_match(line) || gates().reference.is_match(line)
}
pub(super) fn marker_delta(delta: &str) -> bool {
    delta.chars().any(|c| {
        matches!(c, '\n' | '\r' | '\\' | '[' | '`' | '<' | '!' | '*' | '_' | '~' | '$' | '#' | '&' | '@' | '\x1b')
    })
}
pub(super) fn inline_has_open(tokens: &[Token]) -> bool {
    tokens.iter().any(|token| match token.kind {
        Kind::Math { .. } | Kind::Code => false,
        Kind::Html | Kind::HtmlBlock => true,
        Kind::Text => gates().literal.is_match(&token.text) || gates().underscore.is_match(&token.text),
        _ => inline_has_open(&token.children),
    })
}
fn tail32(raw: &str) -> &str {
    let start = raw.char_indices().rev().nth(31).map(|(i, _)| i).unwrap_or(0);
    &raw[start..]
}
pub(super) fn seam_safe(raw: &str, decode: impl Fn(&str) -> String) -> bool {
    let g = gates();
    if g.run_end.is_match(raw) || g.swatch.is_match(raw) || g.entity.is_match(raw) || g.url_prefix.is_match(raw) {
        return false;
    }
    let tail = tail32(raw);
    let decoded = decode(tail);
    if decoded != tail && (g.run_end.is_match(&decoded) || g.swatch.is_match(&decoded)) {
        return false;
    }
    if raw.ends_with(']')
        || raw.rfind('[') > raw.rfind(']')
        || raw.rfind('(') > raw.rfind(')')
        || raw.rfind('<') > raw.rfind('>')
    {
        return false;
    }
    true
}
pub(super) fn delta_safe(
    raw: &str,
    delta: &str,
    tabs: &str,
    has_open: bool,
    delta_tokens: &[Token],
    decode: impl Fn(&str) -> String,
) -> bool {
    if !seam_safe(raw, decode) || delta.contains(['\n', '\r', '\x1b']) {
        return false;
    }
    let marked = marker_delta(delta);
    let last = raw.rsplit('\n').next().unwrap_or(raw);
    let grown = format!("{last}{tabs}");
    if fast_line_start_hazard(&grown) || gates().table.is_match(&grown) {
        return false;
    }
    if marked && (tabs.starts_with('_') || tabs.ends_with('_')) && gates().underscore_word.is_match(raw) {
        return false;
    }
    if !marked && raw.ends_with(['*', '~', '_']) && gates().cmark_word.is_match(tabs) {
        return false;
    }
    if !marked && raw.ends_with('$') && tabs.starts_with(|c: char| c.is_ascii_digit()) {
        return false;
    }
    if marked {
        let pair = (tabs.starts_with(['*', '~', '`']) && raw.ends_with(['*', '~', '`']))
            || (tabs.starts_with('[') && raw.ends_with('!'));
        if has_open || pair || inline_has_open(delta_tokens) {
            return false;
        }
    }
    let seam = raw.rsplit([' ', '\t']).next().unwrap_or(raw);
    !gates().url.is_match(&format!("{seam}{tabs}"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RenderSignature(pub String);
#[derive(Clone)]
pub(super) struct PrefixRows {
    pub signature: RenderSignature,
    pub text: String,
    pub count: usize,
    pub rows: Vec<String>,
}
#[derive(Clone)]
pub(super) struct TailEntry {
    pub raw: String,
    pub kind: Kind,
    pub next: Option<Kind>,
    pub rows: Vec<String>,
}
#[derive(Clone)]
pub(super) struct TailRows {
    pub signature: RenderSignature,
    pub start: usize,
    pub entries: Vec<Option<TailEntry>>,
}
#[derive(Clone)]
pub(super) struct FastTail {
    pub signature: RenderSignature,
    pub source: String,
    pub width: usize,
    pub rows: Arc<Vec<String>>,
    pub row_text: String,
    pub row_raw: String,
    pub row_start: usize,
    pub row_end: usize,
    pub open: bool,
}
#[derive(Clone, Default)]
pub(super) struct StreamState {
    pub prefix_text: String,
    pub prefix_tokens: Vec<Token>,
    pub prefix_rows: Option<PrefixRows>,
    pub tail_rows: Option<TailRows>,
    pub fast_tail: Option<FastTail>,
    pub scan_text: String,
    pub scan_can_stream: bool,
    pub scan_valid: bool,
    pub append_only: bool,
    pub lexed_bytes: u64,
    pub reused_prefix_tokens: u64,
    pub reused_prefix_rows: u64,
    pub reused_tail_rows: u64,
}
impl StreamState {
    pub fn mode_change(&mut self) {
        self.scan_valid = false;
        self.append_only = false;
        self.fast_tail = None;
    }
    pub fn text_change(&mut self, old: &str, new: &str) {
        if !new.starts_with(old) {
            self.append_only = false;
        }
        if new.trim().is_empty() {
            self.clear_prefix();
            self.fast_tail = None;
        }
    }
    pub fn clear_prefix(&mut self) {
        self.prefix_text.clear();
        self.prefix_tokens.clear();
        self.prefix_rows = None;
        self.tail_rows = None;
    }
    pub fn lex(&mut self, text: &str, original_has_cr: bool) -> Vec<Token> {
        let append = self.scan_valid && self.append_only && text.starts_with(&self.scan_text);
        let suffix = if append { &text[self.scan_text.len()..] } else { text };
        let suspicious = suffix.contains(['[', ']', ':', '\n', '\r']);
        let can = if append && !self.scan_can_stream {
            false
        } else if append && !suspicious && !original_has_cr {
            true
        } else {
            !original_has_cr && !gates().reference.is_match(text)
        };
        self.scan_text = text.into();
        self.scan_can_stream = can;
        self.scan_valid = true;
        self.append_only = true;
        let preserve = can && !self.prefix_text.is_empty() && text.starts_with(&self.prefix_text);
        let tokens = if preserve {
            let offset = self.prefix_text.len();
            self.lexed_bytes += (text.len() - offset) as u64;
            self.reused_prefix_tokens += self.prefix_tokens.len() as u64;
            let mut tail = markdown_lexer::lex_document(&text[offset..]);
            shift(&mut tail, offset);
            let mut all = self.prefix_tokens.clone();
            all.extend(tail);
            all
        } else {
            self.lexed_bytes += text.len() as u64;
            markdown_lexer::lex_document(text)
        };
        if can {
            let skip = if preserve { self.prefix_tokens.len() } else { 0 };
            let (end, count) = stable_boundary(text, &tokens, skip);
            if count > 0 {
                self.prefix_text = text[..end].into();
                self.prefix_tokens = tokens[..count].to_vec();
            } else if !preserve {
                self.clear_prefix();
            }
        } else {
            self.clear_prefix();
        }
        // Keep the whole current token list; metadata caches never alter tokens.
        tokens
    }
}
fn shift(tokens: &mut [Token], offset: usize) {
    for t in tokens {
        t.start += offset;
        t.end += offset;
        shift(&mut t.children, offset);
    }
}
fn list_may_continue(text: &str, tail: usize, raw: &str) -> bool {
    let marker = raw.trim_start_matches(' ');
    let unordered = marker.as_bytes().first().copied().filter(|c| matches!(c, b'*' | b'+' | b'-'));
    let ordered = marker.bytes().take_while(u8::is_ascii_digit).count();
    let delimiter = if unordered.is_none() && ordered > 0 && ordered <= 9 {
        marker.as_bytes().get(ordered).copied().filter(|c| matches!(c, b'.' | b')'))
    } else {
        None
    };
    if unordered.is_none() && delimiter.is_none() {
        return true;
    }
    let bytes = text.as_bytes();
    let mut i = tail;
    while i < bytes.len() && i - tail < 3 && bytes[i] == b' ' {
        i += 1;
    }
    if i == bytes.len() {
        return true;
    }
    if let Some(b) = unordered {
        if bytes[i] != b {
            return false;
        }
        i += 1;
    } else {
        let start = i;
        while i < bytes.len() && i - start < 10 && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == start || i - start > 9 {
            return false;
        }
        if i == bytes.len() {
            return true;
        }
        if Some(bytes[i]) != delimiter {
            return false;
        }
        i += 1;
    }
    i == bytes.len() || matches!(bytes[i], b' ' | b'\t' | b'\n')
}
fn stable_boundary(text: &str, tokens: &[Token], skip: usize) -> (usize, usize) {
    let mut chosen = (0, 0);
    for (i, t) in tokens.iter().enumerate().skip(skip) {
        // Rust adapter exposes exact absolute source spans, including explicit
        // omitted-CommonMark gaps. Never add raw lengths as public Marked does.
        if t.end > text.len() || !text.is_char_boundary(t.end) || !text[..t.end].ends_with("\n\n") {
            continue;
        }
        let previous = i.checked_sub(1).and_then(|n| tokens.get(n));
        if previous.is_some_and(|p| matches!(p.kind, Kind::List(_)) && list_may_continue(text, t.end, &p.raw)) {
            continue;
        }
        chosen = (t.end, i + 1);
    }
    if chosen.1 == 0 || chosen.0 >= text.len() || matches!(text.as_bytes()[chosen.0], b' ' | b'\n') {
        (0, 0)
    } else {
        chosen
    }
}
