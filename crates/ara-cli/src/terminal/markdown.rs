//! Fixed OMP static Markdown rendering. Streaming recipes are separate work.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
#[path = "markdown_html.rs"]
mod html;
use super::{
    box_component::StaticComponent,
    markdown_lexer::{self, Kind, Token},
    markdown_math,
    markdown_stream::{self, FastTail, PrefixRows, RenderSignature, StreamState, TailEntry, TailRows},
    render_utils,
    theme::Theme,
    types::FrameContext,
};
use ara_rpc::WireString;
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, OnceLock},
};
use unicode_segmentation::UnicodeSegmentation;
/// Fixed OMP normalization: only well-formed ST-terminated OSC 8 prefixes
/// become BEL-terminated before Markdown parses the backslash terminator.
fn normalize_osc8_terminators(text: &str) -> String {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"(\x1b\]8;[^\x07\x1b]*)\x1b\\").unwrap())
        .replace_all(text, "${1}\x07")
        .into_owned()
}
/// Normalize before storage and equality checks, retaining every UTF-16 unit.
/// The fixed constructor and setText normalize OSC 8 ST terminators before
/// computing both instance and shared-cache identity.
fn normalize_osc8_wire(text: WireString) -> WireString {
    let src = text.units();
    let mut out = Vec::with_capacity(src.len());
    let mut i = 0;
    while i < src.len() {
        if src[i..].starts_with(&[27, 93, 56, 59]) {
            let mut j = i + 4;
            while j < src.len() && src[j] != 7 && src[j] != 27 {
                j += 1;
            }
            if src[j..].starts_with(&[27, 92]) {
                out.extend_from_slice(&src[i..j]);
                out.push(7);
                i = j + 2;
                continue;
            }
        }
        out.push(src[i]);
        i += 1;
    }
    WireString::from_units(out)
}
fn trailing_osc8_wire(text: &WireString) -> Option<WireString> {
    let units = text.units();
    if let Some(start) = units.windows(4).rposition(|w| w == [27, 93, 56, 59]) {
        let body = &units[start + 4..];
        let cut = body.iter().position(|u| matches!(*u, 7 | 27));
        if cut.is_none() || cut.is_some_and(|at| at + 1 == body.len() && body[at] == 27) {
            return Some(WireString::from_units(units[start..].to_vec()));
        }
    }
    if [&[27, 93, 56, 59][..], &[27, 93, 56][..], &[27, 93][..], &[27][..]].iter().any(|prefix| units.ends_with(prefix))
    {
        let start = units.iter().rposition(|u| *u == 27)?;
        return Some(WireString::from_units(units[start..].to_vec()));
    }
    None
}
pub type StyleFunction = Arc<dyn Fn(&str) -> String + Send + Sync>;
#[derive(Clone, Default)]
pub struct DefaultTextStyle {
    pub color: Option<StyleFunction>,
    pub bg_color: Option<StyleFunction>,
    pub bold: bool,
    pub italic: bool,
    pub strikethrough: bool,
    pub underline: bool,
}
pub type MarkdownStyleFunctions = Arc<HashMap<String, StyleFunction>>;
pub type MermaidResolver = Arc<dyn Fn(&str, usize) -> Option<String> + Send + Sync>;
pub type CodeHighlighter = Arc<dyn Fn(&str, Option<&str>) -> Vec<String> + Send + Sync>;
/// A clone must copy parser state independently for rejected frame snapshots.
pub trait MarkdownHighlightSession: Send + Sync {
    fn push(&mut self, chunk: &str) -> String;
    fn frame_copy(&self) -> Box<dyn MarkdownHighlightSession>;
}
impl Clone for Box<dyn MarkdownHighlightSession> {
    fn clone(&self) -> Self {
        self.frame_copy()
    }
}
impl MarkdownHighlightSession for super::highlight::HighlightStream {
    fn push(&mut self, chunk: &str) -> String {
        super::highlight::HighlightStream::push(self, chunk)
    }
    fn frame_copy(&self) -> Box<dyn MarkdownHighlightSession> {
        Box::new(self.clone())
    }
}
pub type HighlightStreamFactory =
    Arc<dyn Fn(Option<&str>) -> Result<Option<Box<dyn MarkdownHighlightSession>>, String> + Send + Sync>;
#[derive(Clone)]
struct StreamingHighlightCache {
    signature: RenderSignature,
    lang: Option<String>,
    text: String,
    lines: Vec<String>,
    // Native syntax output can leave an ANSI reset after its final newline.
    // Carry that unfinished output into the next push, never emit it as a row.
    pending: String,
    stream: Box<dyn MarkdownHighlightSession>,
}
#[derive(Clone)]
struct TailCapture {
    row_input: String,
    row_raw: String,
    open: bool,
}
#[derive(Clone)]
struct DiffHighlightSession {
    callback: CodeHighlighter,
    lang: Option<String>,
}
impl MarkdownHighlightSession for DiffHighlightSession {
    fn push(&mut self, chunk: &str) -> String {
        let mut out = String::new();
        let mut lines = chunk.split('\n').peekable();
        while let Some(line) = lines.next() {
            if lines.peek().is_some() {
                out.push_str(&(self.callback)(line, self.lang.as_deref()).join("\n"));
                out.push('\n');
            } else if !line.is_empty() {
                out.push_str(&(self.callback)(line, self.lang.as_deref()).join("\n"));
            }
        }
        out
    }
    fn frame_copy(&self) -> Box<dyn MarkdownHighlightSession> {
        Box::new(self.clone())
    }
}

fn autolink_scheme_at(src: &[u16], i: usize) -> bool {
    let at = |n| src.get(n).copied().unwrap_or(0);
    match at(i) | 32 {
        119 => at(i + 1) | 32 == 119 && at(i + 2) | 32 == 119 && at(i + 3) == 46,
        104 => {
            if at(i + 1) | 32 != 116 || at(i + 2) | 32 != 116 || at(i + 3) | 32 != 112 {
                return false;
            }
            let j = i + 4 + usize::from(at(i + 4) | 32 == 115);
            at(j) == 58 && at(j + 1) == 47 && at(j + 2) == 47
        }
        102 => at(i + 1) | 32 == 116 && at(i + 2) | 32 == 112 && at(i + 3) == 58 && at(i + 4) == 47 && at(i + 5) == 47,
        _ => false,
    }
}

/// Fixed OMP scanner. The returned offset is in UTF-16 units, as in charCodeAt.
pub fn autolink_scheme_scan_index(source: &WireString) -> Option<usize> {
    let src = source.units();
    (0..src.len()).find(|&i| matches!(src[i] | 32, 119 | 104 | 102) && autolink_scheme_at(src, i))
}

/// Fixed conservative GFM URL gate: long local-part runs must remain possible.
pub fn url_token_possible(source: &WireString) -> bool {
    let src = source.units();
    if autolink_scheme_at(src, 0) {
        return true;
    }
    let mut i = 0;
    while i < 320 {
        let c = src.get(i).copied().unwrap_or(0);
        if !matches!(c, 97..=122 | 65..=90 | 48..=57 | 46 | 95 | 43 | 45) {
            break;
        }
        i += 1;
    }
    i != 0 && (i >= 320 || src.get(i) == Some(&64))
}
#[derive(Clone)]
pub struct MarkdownOptions {
    pub style_functions: Option<MarkdownStyleFunctions>,
    pub default_style: DefaultTextStyle,
    pub padding_x: usize,
    pub padding_y: usize,
    pub code_block_indent: usize,
    pub hyperlinks: bool,
    pub text_sizing: bool,
    pub image_protocol: Option<String>,
    pub true_color: bool,
    pub background: Option<String>,
    pub tight: bool,
    pub mermaid: Option<MermaidResolver>,
    pub highlighter: Option<CodeHighlighter>,
    pub highlight_stream_factory: Option<HighlightStreamFactory>,
}
impl Default for MarkdownOptions {
    fn default() -> Self {
        Self {
            style_functions: None,
            default_style: DefaultTextStyle::default(),
            padding_x: 0,
            padding_y: 0,
            code_block_indent: 2,
            hyperlinks: true,
            text_sizing: false,
            image_protocol: None,
            true_color: true,
            background: None,
            tight: false,
            mermaid: None,
            highlighter: None,
            highlight_stream_factory: None,
        }
    }
}
impl MarkdownOptions {
    /// Frame capabilities have one Host owner. Other rendering options and
    /// callback identities survive a capability update.
    pub(crate) fn set_frame_capabilities(&mut self, frame: &FrameContext) -> bool {
        let image = frame.image_protocol.map(|p| p.as_sequence_prefix().to_owned());
        if (self.true_color, self.text_sizing, self.hyperlinks, &self.image_protocol)
            == (frame.true_color, frame.text_sizing, frame.hyperlinks, &image)
        {
            return false;
        }
        self.true_color = frame.true_color;
        self.text_sizing = frame.text_sizing;
        self.hyperlinks = frame.hyperlinks;
        self.image_protocol = image;
        true
    }
}
#[derive(Clone)]
struct Cache {
    width: usize,
    rows: Arc<Vec<String>>,
}
struct SharedEntry {
    key: String,
    rows: Arc<Vec<String>>,
    size: usize,
}
static SHARED: OnceLock<Mutex<VecDeque<SharedEntry>>> = OnceLock::new();
static EMPTY: OnceLock<Arc<Vec<String>>> = OnceLock::new();
fn shared() -> &'static Mutex<VecDeque<SharedEntry>> {
    SHARED.get_or_init(|| Mutex::new(VecDeque::new()))
}
pub fn clear_render_cache() {
    shared().lock().unwrap_or_else(|e| e.into_inner()).clear();
}
fn shared_get(key: &str) -> Option<Arc<Vec<String>>> {
    let mut entries = shared().lock().unwrap_or_else(|e| e.into_inner());
    let index = entries.iter().position(|e| e.key == key)?;
    let e = entries.remove(index)?;
    let rows = e.rows.clone();
    entries.push_back(e);
    Some(rows)
}
fn shared_put(key: String, rows: Arc<Vec<String>>) {
    let size = rows.len() + rows.iter().map(|l| l.encode_utf16().count()).sum::<usize>();
    if size > 256 * 1024 {
        return;
    }
    let mut entries = shared().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(i) = entries.iter().position(|e| e.key == key) {
        entries.remove(i);
    }
    while entries.len() >= 256 || entries.iter().map(|e| e.size).sum::<usize>() + size > 4 * 1024 * 1024 {
        entries.pop_front();
    }
    entries.push_back(SharedEntry { key, rows, size });
}
pub struct MarkdownComponent {
    source: WireString,
    osc_partial_escape: Option<WireString>,
    theme: Arc<Theme>,
    color: Option<String>,
    options: MarkdownOptions,
    tokens: Option<Vec<Token>>,
    cache: Option<Cache>,
    ignore_tight: bool,
    transient_render_cache: bool,
    default_style_prefix: OnceLock<String>,
    stream: StreamState,
    active_signature: Option<RenderSignature>,
    rendering_stable_prefix: bool,
    highlight_cache: Mutex<Option<StreamingHighlightCache>>,
    tail_capture: Mutex<Option<TailCapture>>,
}
#[derive(Clone)]
struct Row {
    text: String,
    literal: bool,
    nested: bool,
    wrapped: bool,
}
impl Row {
    fn new(text: impl Into<String>) -> Self {
        Self { text: text.into(), literal: false, nested: false, wrapped: false }
    }
}
impl MarkdownComponent {
    /// Retained component state is committed only with its accepted frame.
    pub(crate) fn frame_copy(&self) -> Self {
        Self {
            source: self.source.clone(),
            osc_partial_escape: self.osc_partial_escape.clone(),
            theme: self.theme.clone(),
            color: self.color.clone(),
            options: self.options.clone(),
            tokens: self.tokens.clone(),
            cache: self.cache.clone(),
            ignore_tight: self.ignore_tight,
            transient_render_cache: self.transient_render_cache,
            default_style_prefix: self.default_style_prefix.clone(),
            stream: self.stream.clone(),
            active_signature: None,
            rendering_stable_prefix: false,
            highlight_cache: Mutex::new(self.highlight_cache.lock().unwrap_or_else(|e| e.into_inner()).clone()),
            tail_capture: Mutex::new(None),
        }
    }
    pub(crate) fn set_frame_capabilities(&mut self, frame: &FrameContext) -> bool {
        if !self.options.set_frame_capabilities(frame) {
            return false;
        }
        self.cache = None;
        true
    }
    pub fn new(text: WireString, theme: Arc<Theme>) -> Self {
        let source = normalize_osc8_wire(text);
        let osc_partial_escape = trailing_osc8_wire(&source);
        Self {
            source,
            osc_partial_escape,
            theme,
            color: None,
            options: MarkdownOptions::default(),
            tokens: None,
            cache: None,
            ignore_tight: false,
            transient_render_cache: false,
            default_style_prefix: OnceLock::new(),
            stream: StreamState::default(),
            active_signature: None,
            rendering_stable_prefix: false,
            highlight_cache: Mutex::new(None),
            tail_capture: Mutex::new(None),
        }
    }
    pub fn new_with_color(text: WireString, theme: Arc<Theme>, color: &str) -> Self {
        let mut c = Self::new(text, theme);
        c.color = Some(color.into());
        c
    }
    pub fn set_options(&mut self, options: MarkdownOptions) {
        self.options = options;
        self.default_style_prefix = OnceLock::new();
        self.cache = None;
    }
    pub fn invalidate(&mut self, theme: Arc<Theme>) {
        if !Arc::ptr_eq(&self.theme, &theme) {
            self.default_style_prefix = OnceLock::new();
        }
        self.theme = theme;
        self.cache = None;
    }
    /// Fixed transient/final normalization and L2 admission. Incremental lex
    /// and splice recipes remain separate, unaccepted work.
    pub fn set_transient_render_cache(&mut self, value: bool) {
        if self.transient_render_cache == value {
            return;
        }
        self.transient_render_cache = value;
        self.stream.mode_change();
        self.tokens = None;
        self.cache = None;
    }
    pub fn set_text(&mut self, text: WireString) -> bool {
        if text == self.source {
            return false;
        }
        let text = if text.units().len() > self.source.units().len() && text.units().starts_with(self.source.units()) {
            let pending = self.osc_partial_escape.as_ref().map(WireString::units).unwrap_or(&[]);
            let pending_len = pending.len();
            let mut region = pending.to_vec();
            region.extend_from_slice(&text.units()[self.source.units().len()..]);
            let region = normalize_osc8_wire(WireString::from_units(region));
            self.osc_partial_escape = trailing_osc8_wire(&region);
            let mut units = self.source.units()[..self.source.units().len() - pending_len].to_vec();
            units.extend_from_slice(region.units());
            WireString::from_units(units)
        } else {
            let normalized = normalize_osc8_wire(text);
            self.osc_partial_escape = trailing_osc8_wire(&normalized);
            normalized
        };
        if self.source == text {
            return false;
        }
        self.stream
            .text_change(&String::from_utf16_lossy(self.source.units()), &String::from_utf16_lossy(text.units()));
        self.source = text;
        self.tokens = None;
        self.cache = None;
        true
    }
    fn render_signature(&self, width: usize) -> RenderSignature {
        let styles = self.options.style_functions.as_ref().map(|f| Arc::as_ptr(f) as usize).unwrap_or(0);
        let default = &self.options.default_style;
        let default_key = format!(
            "{}:{}:{}:{}:{}:{}:{:?}",
            default.color.as_ref().map(|f| Arc::as_ptr(f) as *const () as usize).unwrap_or(0),
            default.bg_color.as_ref().map(|f| Arc::as_ptr(f) as *const () as usize).unwrap_or(0),
            default.bold,
            default.italic,
            default.strikethrough,
            default.underline,
            (
                default.bg_color.as_ref().map(|f| f("\x01")),
                self.options.style_functions.as_ref().and_then(|m| m.get("heading")).map(|f| f(""))
            )
        );
        let factory_id =
            self.options.highlight_stream_factory.as_ref().map(|f| Arc::as_ptr(f) as *const () as usize).unwrap_or(0);
        RenderSignature(format!(
            "{styles}\0{default_key}\0{factory_id}\0{width}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{:?}\0{:?}\0{:?}\0{}\0{}\0{}\0{}",
            self.theme.identity,
            self.theme.revision,
            self.options.padding_x,
            self.options.padding_y,
            self.options.code_block_indent,
            self.options.hyperlinks,
            self.options.text_sizing,
            self.options.true_color,
            self.options.background,
            self.options.image_protocol,
            self.color,
            self.options.tight,
            self.ignore_tight,
            self.options.mermaid.as_ref().map(|f| Arc::as_ptr(f) as *const () as usize).unwrap_or(0),
            self.options.highlighter.as_ref().map(|f| Arc::as_ptr(f) as *const () as usize).unwrap_or(0)
        ))
    }
    fn cache_key(&self, signature: &RenderSignature, normalized: &str) -> String {
        let mut source = normalized.encode_utf16().map(|u| format!("{u:04x}")).collect::<String>();
        source.push('\0');
        if char::decode_utf16(self.source.units().iter().copied()).any(|scalar| scalar.is_err()) {
            source.extend(self.source.units().iter().map(|unit| format!("{unit:04x}")));
        }
        format!("{}\0{}", signature.0, source)
    }
    pub fn last_render_stable_text(&self) -> &str {
        if self.transient_render_cache { &self.stream.prefix_text } else { "" }
    }
    pub fn render(&mut self, width: usize) -> Arc<Vec<String>> {
        if let Some(cache) = &self.cache
            && cache.width == width
        {
            return cache.rows.clone();
        }
        if String::from_utf16_lossy(self.source.units()).trim().is_empty() {
            let rows = EMPTY.get_or_init(|| Arc::new(vec![])).clone();
            self.cache = Some(Cache { width, rows: rows.clone() });
            return rows;
        }
        let tabbed = render_utils::replace_tabs(
            &normalize_osc8_terminators(&String::from_utf16_lossy(self.source.units()))
                .replace("\r\n", "\n")
                .replace('\r', "\n"),
        );
        let normalized = if self.transient_render_cache { tabbed } else { repair_orphan_fence(&tabbed) };
        let signature = self.render_signature(width);
        *self.tail_capture.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let cache_key = self.cache_key(&signature, &normalized);
        let padding_x = if self.options.tight && !self.ignore_tight {
            self.options.padding_x.saturating_sub(1)
        } else {
            self.options.padding_x
        };
        let content_width = width.saturating_sub(padding_x * 2).max(1);
        if self.transient_render_cache {
            if let Some(rows) = self.try_fast_tail(width, padding_x, content_width, &signature) {
                return rows;
            }
        }
        if !self.transient_render_cache
            && let Some(rows) = shared_get(&cache_key)
        {
            self.cache = Some(Cache { width, rows: rows.clone() });
            return rows;
        }
        if self.tokens.is_none() {
            let original_has_cr = self.source.units().contains(&13);
            self.tokens = Some(self.stream.lex(&normalized, original_has_cr));
        }
        self.active_signature = Some(signature.clone());
        let tokens = self.tokens.as_ref().unwrap();
        let stable_count = if self.transient_render_cache && normalized.starts_with(&self.stream.prefix_text) {
            self.stream.prefix_tokens.len()
        } else {
            0
        };
        let mut content = Vec::new();
        let previous_prefix = self.stream.prefix_rows.take().filter(|cache| {
            cache.signature == signature
                && normalized.starts_with(&cache.text)
                && self.stream.prefix_text.starts_with(&cache.text)
                && cache.count <= stable_count
        });
        let mut start = 0;
        if let Some(cache) = previous_prefix {
            start = cache.count;
            self.stream.reused_prefix_rows += cache.rows.len() as u64;
            content.extend(cache.rows);
        }
        self.rendering_stable_prefix = true;
        for i in start..stable_count {
            content.extend(self.token_rows(
                &tokens[i],
                tokens.get(i + 1).map(|t| &t.kind),
                width,
                padding_x,
                content_width,
            ));
        }
        self.rendering_stable_prefix = false;
        if stable_count > 0 {
            self.stream.prefix_rows = Some(PrefixRows {
                signature: signature.clone(),
                text: self.stream.prefix_text.clone(),
                count: stable_count,
                rows: content.clone(),
            });
        }
        let old_tail =
            self.stream.tail_rows.take().filter(|cache| cache.signature == signature && cache.start == stable_count);
        let mut entries = Vec::with_capacity(tokens.len().saturating_sub(stable_count));
        let mut contiguous = self.transient_render_cache;
        for i in stable_count..tokens.len() {
            let token = &tokens[i];
            let next = tokens.get(i + 1).map(|t| &t.kind);
            let previous =
                old_tail.as_ref().and_then(|cache| cache.entries.get(i - stable_count)).and_then(Option::as_ref);
            let cached = previous.filter(|entry| {
                contiguous && entry.raw == token.raw && entry.kind == token.kind && entry.next.as_ref() == next
            });
            let rows = if let Some(entry) = cached {
                self.stream.reused_tail_rows += entry.rows.len() as u64;
                entry.rows.clone()
            } else {
                contiguous = false;
                self.token_rows(token, next, width, padding_x, content_width)
            };
            entries.push(if matches!(token.kind, Kind::Table) {
                None
            } else {
                Some(TailEntry {
                    raw: token.raw.clone(),
                    kind: token.kind.clone(),
                    next: next.cloned(),
                    rows: rows.clone(),
                })
            });
            content.extend(rows);
        }
        if self.transient_render_cache {
            self.stream.tail_rows = Some(TailRows { signature: signature.clone(), start: stable_count, entries });
        }
        self.active_signature = None;
        let blank = self
            .options
            .default_style
            .bg_color
            .as_ref()
            .map(|bg| apply_background(&" ".repeat(width), width, bg))
            .unwrap_or_else(|| " ".repeat(width));
        let mut rows = vec![blank.clone(); self.options.padding_y];
        rows.extend(content);
        rows.extend(vec![blank; self.options.padding_y]);
        let rows = Arc::new(rows);
        self.capture_fast_tail(&normalized, width, padding_x, content_width, &signature, rows.clone());
        if !self.transient_render_cache {
            shared_put(cache_key, rows.clone());
        }
        self.cache = Some(Cache { width, rows: rows.clone() });
        rows
    }
    fn token_rows(
        &self,
        token: &Token,
        next: Option<&Kind>,
        width: usize,
        padding_x: usize,
        content_width: usize,
    ) -> Vec<String> {
        let mut content = Vec::new();
        *self.tail_capture.lock().unwrap_or_else(|e| e.into_inner()) = None;
        for row in self.block(token, content_width, next, false) {
            let rendered = if row.wrapped || row.text.contains("\x1b]66;") {
                vec![row.text.clone()]
            } else {
                render_utils::wrap(&row.text, content_width)
            };
            if token.kind == Kind::Paragraph
                && !row.literal
                && !row.wrapped
                && !row.text.is_empty()
                && display_body(&token.children).is_none()
                && tree_wrap(&row.text, content_width).is_none()
            {
                *self.tail_capture.lock().unwrap_or_else(|e| e.into_inner()) = Some(TailCapture {
                    row_input: rendered.last().cloned().unwrap_or_default(),
                    row_raw: token.raw.rsplit('\n').next().unwrap_or(&token.raw).into(),
                    open: markdown_stream::inline_has_open(&token.children),
                });
            }
            for line in rendered {
                if row.literal || line.contains("\x1b]66;") {
                    content.push(line);
                } else {
                    let margin = " ".repeat(padding_x);
                    let mut full = format!("{margin}{line}{margin}");
                    full.push_str(&" ".repeat(width.saturating_sub(render_utils::width(&full))));
                    if let Some(bg) = &self.options.background {
                        let code = self.theme.bg_ansi(bg);
                        full = format!(
                            "{code}{}\x1b[49m",
                            full.replace("\x1b[0m", &format!("\x1b[0m{code}"))
                                .replace("\x1b[49m", &format!("\x1b[49m{code}"))
                        );
                    }
                    if let Some(bg) = &self.options.default_style.bg_color {
                        full = apply_background(&full, width, bg);
                    }
                    content.push(full);
                }
            }
        }
        content
    }
    fn capture_fast_tail(
        &mut self,
        _normalized: &str,
        _width: usize,
        _padding_x: usize,
        content_width: usize,
        signature: &RenderSignature,
        rows: Arc<Vec<String>>,
    ) {
        self.stream.fast_tail = None;
        let style = &self.options.default_style;
        if !self.transient_render_cache
            || style.color.is_some()
            || style.bold
            || style.italic
            || style.strikethrough
            || style.underline
        {
            return;
        }
        // Fixed capture hook only admits the final token of the final render
        // range, never a newline-terminated paragraph whose next append starts
        // a fresh physical line.
        if !self
            .tokens
            .as_ref()
            .and_then(|tokens| tokens.last())
            .is_some_and(|token| token.kind == Kind::Paragraph && !token.raw.ends_with('\n'))
        {
            return;
        }
        let capture = self.tail_capture.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(capture) = capture {
            let row_end = rows.len().saturating_sub(self.options.padding_y);
            if row_end <= self.options.padding_y || capture.row_input.contains("\x1b]") {
                return;
            }
            self.stream.fast_tail = Some(FastTail {
                signature: signature.clone(),
                source: String::from_utf16_lossy(self.source.units()),
                width: content_width,
                rows,
                row_text: capture.row_input,
                row_raw: capture.row_raw,
                row_start: row_end - 1,
                row_end,
                open: capture.open,
            });
        }
    }
    fn try_fast_tail(
        &mut self,
        width: usize,
        padding_x: usize,
        content_width: usize,
        signature: &RenderSignature,
    ) -> Option<Arc<Vec<String>>> {
        let recipe = self.stream.fast_tail.take()?;
        let source = String::from_utf16_lossy(self.source.units());
        if recipe.signature != *signature
            || recipe.width != content_width
            || source.len() <= recipe.source.len()
            || !source.starts_with(&recipe.source)
        {
            return None;
        }
        let delta = &source[recipe.source.len()..];
        let tabs = render_utils::replace_tabs(delta);
        let marked = markdown_stream::marker_delta(delta);
        // Same-line deltas have no block structure. Use the existing inline
        // adapter's paragraph tokens; non-paragraph shapes fall back safely.
        let delta_roots = if marked { markdown_lexer::lex_document(&tabs) } else { Vec::new() };
        if marked && (delta_roots.len() != 1 || delta_roots[0].kind != Kind::Paragraph) {
            return None;
        }
        let delta_tokens = delta_roots.first().map(|t| t.children.as_slice()).unwrap_or(&[]);
        if !markdown_stream::delta_safe(&recipe.row_raw, delta, &tabs, recipe.open, delta_tokens, html::entities) {
            return None;
        }
        let added = if marked {
            // The block lexer used by this inline adapter consumes paragraph
            // indentation. Fixed OMP lexInlineTokens preserves these source
            // spaces: appending a styled delta must not join adjacent words.
            let leading = tabs.len() - tabs.trim_start_matches(' ').len();
            format!("{}{}", self.default_text(&tabs[..leading], false), self.inline(delta_tokens, false))
        } else {
            self.default_text(&html::entities(&tabs), false)
        };
        let grown = format!("{}{added}", recipe.row_text);
        let wrapped = render_utils::wrap(&grown, content_width);
        let margin = " ".repeat(padding_x);
        let mut replacements = Vec::new();
        for row in &wrapped {
            let mut full = format!("{margin}{row}{margin}");
            full.push_str(&" ".repeat(width.saturating_sub(render_utils::width(&full))));
            if let Some(background) = &self.options.background {
                let code = self.theme.bg_ansi(background);
                full = format!(
                    "{code}{}\x1b[49m",
                    full.replace("\x1b[0m", &format!("\x1b[0m{code}")).replace("\x1b[49m", &format!("\x1b[49m{code}"))
                );
            }
            if let Some(background) = &self.options.default_style.bg_color {
                full = apply_background(&full, width, background);
            }
            replacements.push(full);
        }
        let replacement_len = replacements.len();
        if replacement_len == 0 {
            return None;
        }
        let mut output = recipe.rows[..recipe.row_start].to_vec();
        output.extend(replacements);
        output.extend_from_slice(&recipe.rows[recipe.row_end..]);
        let rows = Arc::new(output);
        self.stream.fast_tail = Some(FastTail {
            rows: rows.clone(),
            source,
            width: recipe.width,
            signature: recipe.signature,
            row_text: wrapped.last().cloned().unwrap_or_default(),
            row_raw: format!("{}{}", recipe.row_raw, tabs),
            row_start: recipe.row_start + replacement_len - 1,
            row_end: recipe.row_start + replacement_len,
            open: recipe.open,
        });
        // This is after the real existing-row replacement, not an arm/fallback.
        markdown_stream::record_actual_splice();
        self.cache = Some(Cache { width, rows: rows.clone() });
        Some(rows)
    }
    fn highlight_streaming_lines(&self, completed: &str, lang: Option<&str>) -> Option<Vec<String>> {
        let signature = self.active_signature.as_ref()?;
        let mut state = self.highlight_cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(cache) = state.as_mut().filter(|cache| {
            cache.signature == *signature
                && cache.lang.as_deref() == lang
                && completed.starts_with(&cache.text)
                && (completed.len() == cache.text.len() || completed.as_bytes().get(cache.text.len()) == Some(&b'\n'))
        }) {
            if completed.len() == cache.text.len() {
                return Some(cache.lines.clone());
            }
            let chunk = format!("{}\n", &completed[cache.text.len() + 1..]);
            let pushed = format!("{}{}", cache.pending, cache.stream.push(&chunk));
            let (lines, pending) = split_highlight_lines(&pushed);
            cache.lines.extend(lines);
            cache.pending = pending;
            cache.text = completed.into();
            return Some(cache.lines.clone());
        }
        let produced = if let Some(factory) = &self.options.highlight_stream_factory {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| factory(lang))) {
                // Fixed present factory success(null) returns immediately. Only
                // absent/error/panic factory may reach the diff-family fallback.
                Ok(Ok(None)) => return None,
                Ok(Ok(Some(stream))) => Some(stream),
                Ok(Err(_)) | Err(_) => None,
            }
        } else { None };
        let mut stream = if let Some(stream) = produced {
            stream
        } else {
            if !lang.is_some_and(|lang| matches!(lang.to_ascii_lowercase().as_str(), "diff" | "patch" | "udiff")) {
                return None;
            }
            Box::new(DiffHighlightSession {
                callback: self.options.highlighter.as_ref()?.clone(),
                lang: lang.map(str::to_owned),
            })
        };
        let pushed = stream.push(&format!("{completed}\n"));
        let (lines, pending) = split_highlight_lines(&pushed);
        *state = Some(StreamingHighlightCache {
            signature: signature.clone(),
            lang: lang.map(str::to_owned),
            text: completed.into(),
            lines: lines.clone(),
            pending,
            stream,
        });
        Some(lines)
    }
    fn code_body_lines(&self, token: &Token, code: &str, lang: Option<&str>) -> Vec<String> {
        if let Some(highlighter) = &self.options.highlighter {
            if !self.transient_render_cache || self.rendering_stable_prefix || code_has_closing_fence(&token.raw) {
                return highlighter(code, lang);
            }
            if let Some(line_end) = code.rfind('\n') {
                if let Some(mut lines) = self.highlight_streaming_lines(&code[..line_end], lang) {
                    lines.extend(code[line_end + 1..].split('\n').map(|line| self.fg("mdCodeBlock", line)));
                    return lines;
                }
            }
        }
        code.split('\n').map(|line| self.fg("mdCodeBlock", line)).collect()
    }
    fn fg(&self, key: &str, text: &str) -> String {
        let name = match key {
            "mdHeading" => "heading",
            "mdLink" => "link",
            "mdLinkUrl" => "linkUrl",
            "mdCode" => "code",
            "mdCodeBlock" => "codeBlock",
            "mdCodeBlockBorder" => "codeBlockBorder",
            "mdQuote" => "quote",
            "mdQuoteBorder" => "quoteBorder",
            "mdHr" => "hr",
            "mdListBullet" => "listBullet",
            _ => key,
        };
        if let Some(f) = self.options.style_functions.as_ref().and_then(|m| m.get(name)) {
            return f(text);
        }
        if text.is_empty() {
            return String::new();
        }
        let prefix = self.theme.ansi(key);
        format!("{prefix}{}\x1b[39m", text.replace("\x1b[39m", prefix).replace("\x1b[0m", &format!("\x1b[0m{prefix}")))
    }
    fn attr(&self, text: &str, on: u8, off: u8) -> String {
        let name = match on {
            1 => "bold",
            3 => "italic",
            9 => "strikethrough",
            4 => "underline",
            _ => "",
        };
        self.options
            .style_functions
            .as_ref()
            .and_then(|m| m.get(name))
            .map(|f| f(text))
            .unwrap_or_else(|| style(text, on, off))
    }
    fn default_text(&self, text: &str, quote: bool) -> String {
        if quote {
            return text.into();
        }
        text.split('\n')
            .map(|run| {
                if run.is_empty() {
                    return String::new();
                }
                let d = &self.options.default_style;
                let mut styled = d
                    .color
                    .as_ref()
                    .map(|f| f(run))
                    .unwrap_or_else(|| self.color.as_ref().map(|c| self.fg(c, run)).unwrap_or_else(|| run.into()));
                for (enabled, on, off) in
                    [(d.bold, 1, 22), (d.italic, 3, 23), (d.strikethrough, 9, 29), (d.underline, 4, 24)]
                {
                    if enabled {
                        styled = self.attr(&styled, on, off);
                    }
                }
                styled
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
    fn default_prefix(&self, quote: bool) -> String {
        if quote {
            return String::new();
        }
        self.default_style_prefix
            .get_or_init(|| {
                let styled = self.default_text("\0", false);
                styled.find('\0').map(|i| styled[..i].to_owned()).unwrap_or_default()
            })
            .clone()
    }
    fn link(&self, text: String, target: &str) -> String {
        let target = target.replace(['\x1b', '\x07'], "");
        if !self.options.hyperlinks || target.is_empty() {
            text
        } else {
            format!("\x1b]8;;{target}\x07{text}\x1b]8;;\x07")
        }
    }
    fn inline(&self, tokens: &[Token], quote: bool) -> String {
        let mut out = String::new();
        let prefix = self.default_prefix(quote);
        let mut state = html::HtmlState::default();
        let mut trim_leading = false;
        let mut i = 0;
        while i < tokens.len() {
            let token = &tokens[i];
            let nested = || self.inline(&token.children, quote);
            match &token.kind {
                Kind::Math { .. } => {
                    state.mark(&token.text);
                    out.push_str(
                        &self.default_text(
                            &markdown_math::latex_to_unicode_with_color(&token.text, self.options.true_color)
                                .replace('\n', " "),
                            quote,
                        ),
                    );
                }
                Kind::Text | Kind::SoftBreak => {
                    let text = if trim_leading { token.text.trim_start() } else { &token.text };
                    trim_leading = false;
                    let text = html::entities(text);
                    state.mark(&text);
                    if token.children.is_empty() {
                        out.push_str(&self.text_swatches(&text, quote));
                    } else {
                        out.push_str(&nested());
                    }
                }
                Kind::Paragraph => out.push_str(&nested()),
                Kind::Strong => {
                    out.push_str(&self.attr(&nested(), 1, 22));
                    out.push_str(&prefix);
                }
                Kind::Em => {
                    out.push_str(&self.attr(&nested(), 3, 23));
                    out.push_str(&prefix);
                }
                Kind::Del => {
                    out.push_str(&self.attr(&nested(), 9, 29));
                    out.push_str(&prefix);
                }
                Kind::Code => {
                    state.mark(&token.text);
                    if let Some(swatch) = self.code_swatch(&token.text) {
                        out.push_str(&swatch);
                    } else {
                        out.push_str(&self.fg("mdCode", &token.text));
                    }
                    out.push_str(&prefix);
                }
                Kind::Link(href) => {
                    state.mark(&markdown_lexer::plain_inline(&token.children));
                    let text = nested();
                    let plain = markdown_lexer::plain_inline(&token.children);
                    out.push_str(&self.link(self.fg("mdLink", &self.attr(&text, 4, 24)), href));
                    if !href.is_empty() && plain != *href && plain != href.strip_prefix("mailto:").unwrap_or(href) {
                        out.push(' ');
                        out.push_str(&self.link(self.fg("mdLinkUrl", &format!("({href})")), href));
                    }
                    out.push_str(&prefix);
                }
                Kind::Image(_) => {
                    out.push_str(&self.default_text(&markdown_lexer::plain_inline(&token.children), quote))
                }
                Kind::Br => {
                    out.push('\n');
                    trim_leading = true;
                }
                Kind::Html => {
                    let raw = token.text.as_str();
                    if raw.eq_ignore_ascii_case("<code>") {
                        if let Some(end) = tokens[i + 1..]
                            .iter()
                            .position(|t| t.kind == Kind::Html && t.text.eq_ignore_ascii_case("</code>"))
                        {
                            let content = html::entities(&markdown_lexer::plain_inline(&tokens[i + 1..i + 1 + end]));
                            out.push_str(&self.fg("mdCode", &content));
                            out.push_str(&prefix);
                            i += end + 2;
                            continue;
                        }
                    }
                    let cleaned = html::normalize(raw, &mut state, None);
                    out.push_str(&self.default_text(&cleaned, quote));
                    if cleaned.ends_with('\n') {
                        trim_leading = true;
                    } else if !cleaned.is_empty() {
                        trim_leading = false;
                    }
                }
                Kind::Checkbox(checked) => out.push_str(if *checked { "[x] " } else { "[ ] " }),
                _ => {
                    if token.children.is_empty() {
                        out.push_str(&self.default_text(&html::entities(&token.text), quote));
                    } else {
                        out.push_str(&nested());
                    }
                }
            }
            i += 1;
        }
        while !prefix.is_empty() && out.ends_with(&prefix) {
            out.truncate(out.len() - prefix.len());
        }
        out
    }
    fn block(&self, token: &Token, width: usize, next: Option<&Kind>, quote: bool) -> Vec<Row> {
        let mut rows = vec![];
        let spaced = next.is_some_and(|k| *k != Kind::Space);
        match &token.kind {
            Kind::Paragraph => {
                if let Some(body) = display_body(&token.children) {
                    rows.extend(
                        markdown_math::latex_to_block_with_color(&body, self.options.true_color)
                            .iter()
                            .map(|l| Row::new(self.default_text(l, quote))),
                    );
                } else {
                    let paragraph = self.inline(&token.children, quote);
                    rows.extend(
                        tree_wrap(&paragraph, width).unwrap_or_else(|| vec![paragraph]).into_iter().map(Row::new),
                    );
                }
                if spaced && !matches!(next, Some(Kind::List(_))) {
                    rows.push(Row::new(""));
                }
            }
            Kind::Heading(level) => {
                let plain = markdown_lexer::plain_inline(&token.children);
                if *level == 1
                    && self.options.text_sizing
                    && render_utils::width(&plain) > 0
                    && render_utils::width(&plain) * 2 <= width
                {
                    rows.push(Row::new(
                        self.fg("mdHeading", &self.attr(&self.attr(&sized_heading(&plain), 4, 24), 1, 22)),
                    ));
                    let mut reserve = Row::new("");
                    reserve.literal = true;
                    reserve.wrapped = true;
                    rows.push(reserve);
                    if spaced {
                        rows.push(Row::new(""));
                    }
                    return rows;
                }
                let text = self.inline(&token.children, quote);
                let text = if *level == 1 {
                    self.attr(&self.attr(&text, 4, 24), 1, 22)
                } else if *level == 2 {
                    self.attr(&text, 1, 22)
                } else {
                    self.attr(&format!("{} {text}", "#".repeat(*level as usize)), 1, 22)
                };
                rows.push(Row::new(self.fg("mdHeading", &text)));
                if spaced {
                    rows.push(Row::new(""));
                }
            }
            Kind::CodeBlock(lang) => {
                let code = token.children.iter().map(|c| c.text.as_str()).collect::<String>();
                let code = code.strip_suffix('\n').unwrap_or(&code);
                if lang == "mermaid"
                    && let Some(resolver) = &self.options.mermaid
                    && let Some(ascii) = resolver(code, width).filter(|s| !s.is_empty())
                {
                    for line in ascii.split('\n') {
                        let mut row = Row::new(if render_utils::width(line) > width {
                            render_utils::slice(line, 0, width)
                        } else {
                            line.into()
                        });
                        row.wrapped = true;
                        rows.push(row);
                    }
                    if spaced {
                        rows.push(Row::new(""));
                    }
                    return rows;
                }
                rows.push(Row::new(self.fg("mdCodeBlockBorder", &format!("```{lang}"))));
                let highlighted = self.code_body_lines(token, code, (!lang.is_empty()).then_some(lang.as_str()));
                for line in highlighted {
                    let mut row = Row::new(if self.options.code_block_indent == 0 {
                        line
                    } else {
                        format!("{}{line}", " ".repeat(self.options.code_block_indent))
                    });
                    row.literal = self.options.code_block_indent == 0;
                    rows.push(row);
                }
                rows.push(Row::new(self.fg("mdCodeBlockBorder", "```")));
                if spaced {
                    rows.push(Row::new(""));
                }
            }
            Kind::List(start) => rows = self.list(token, *start, 0, width, quote),
            Kind::Table => {
                rows.extend(self.table(token, width, quote).into_iter().map(Row::new));
                if spaced {
                    rows.push(Row::new(""));
                }
            }
            Kind::Quote => {
                let children = &token.children;
                let inner_width = width.saturating_sub(2).max(1);
                let mut inner = vec![];
                for (i, child) in children.iter().enumerate() {
                    inner.extend(self.block(child, inner_width, children.get(i + 1).map(|t| &t.kind), true));
                }
                while inner.last().is_some_and(|r| r.text.is_empty()) {
                    inner.pop();
                }
                rows.extend(self.quote_border(inner, width));
                if spaced {
                    rows.push(Row::new(""));
                }
            }
            Kind::Hr => {
                rows.push(Row::new(self.hr(width, token.raw.trim().chars().next())));
                if spaced {
                    rows.push(Row::new(""));
                }
            }
            Kind::Space => rows.push(Row::new("")),
            Kind::Definition => {}
            Kind::Math { .. } => {
                rows.extend(
                    markdown_math::latex_to_block_with_color(&token.text, self.options.true_color)
                        .iter()
                        .map(|l| Row::new(self.default_text(l, quote))),
                );
                if spaced {
                    rows.push(Row::new(""));
                }
            }
            Kind::HtmlBlock => {
                let html = token.children.iter().map(|t| t.text.as_str()).collect::<String>();
                rows = self.html_block(&html, width, quote);
            }
            _ => rows.push(Row::new(if token.children.is_empty() {
                token.text.clone()
            } else {
                self.inline(&token.children, quote)
            })),
        }
        rows
    }
    fn quote_border(&self, inner: Vec<Row>, width: usize) -> Vec<Row> {
        let quote_style = |text: &str| self.fg("mdQuote", &self.attr(text, 3, 23));
        let probe = quote_style("\0");
        let prefix = probe.find('\0').map(|i| &probe[..i]).unwrap_or("");
        let inner_width = width.saturating_sub(2).max(1);
        let mut rows = Vec::new();
        for row in inner {
            if row.literal {
                for line in render_utils::wrap(&row.text, inner_width) {
                    let mut rendered = Row::new(line);
                    rendered.literal = true;
                    rendered.wrapped = true;
                    rows.push(rendered);
                }
            } else {
                let restored =
                    if prefix.is_empty() { row.text } else { row.text.replace("\x1b[0m", &format!("\x1b[0m{prefix}")) };
                for line in render_utils::wrap(&quote_style(&restored), inner_width) {
                    let border = self.fg("mdQuoteBorder", &format!("{} ", self.theme.symbol("md.quoteBorder")));
                    let mut rendered = Row::new(format!("{border}{line}"));
                    rendered.wrapped = true;
                    rows.push(rendered);
                }
            }
        }
        rows
    }
    fn hr(&self, width: usize, source: Option<char>) -> String {
        let symbol = self.theme.symbol("md.hrChar");
        let ascii = symbol == "-";
        let ch = match source {
            Some('=') => "=",
            Some('═') => {
                if ascii {
                    "="
                } else {
                    "═"
                }
            }
            Some('━') => {
                if ascii {
                    "-"
                } else {
                    "━"
                }
            }
            Some('─') => {
                if ascii {
                    "-"
                } else {
                    "─"
                }
            }
            Some('–') => {
                if ascii {
                    "-"
                } else {
                    "–"
                }
            }
            Some('—') => {
                if ascii {
                    "-"
                } else {
                    "—"
                }
            }
            _ => symbol,
        };
        self.fg("mdHr", &ch.repeat(width.min(80)))
    }
    fn list(&self, token: &Token, start: Option<u64>, depth: usize, width: usize, quote: bool) -> Vec<Row> {
        let indent = "  ".repeat(depth);
        let mut rows = vec![];
        for (index, item) in token.children.iter().filter(|t| t.kind == Kind::Item).enumerate() {
            let bullet = start.map(|n| format!("{}. ", n + index as u64)).unwrap_or_else(|| "- ".into());
            let first = format!("{indent}{}", self.fg("mdListBullet", &bullet));
            let continuation = format!("{indent}{}", " ".repeat(render_utils::width(&bullet)));
            let mut item_rows = vec![];
            for child in &item.children {
                if let Kind::List(start) = child.kind {
                    let mut nested = self.list(child, start, depth + 1, width, quote);
                    for r in &mut nested {
                        r.nested = true;
                    }
                    item_rows.extend(nested);
                } else if matches!(child.kind, Kind::Paragraph | Kind::Text) {
                    if let Some(body) = display_body(&child.children) {
                        item_rows.extend(
                            markdown_math::latex_to_block_with_color(&body, self.options.true_color)
                                .iter()
                                .map(|l| Row::new(self.default_text(l, quote))),
                        );
                    } else {
                        item_rows.push(Row::new(if child.children.is_empty() {
                            child.text.clone()
                        } else {
                            self.inline(&child.children, quote)
                        }));
                    }
                } else if matches!(child.kind, Kind::CodeBlock(_) | Kind::Math { .. }) {
                    item_rows.extend(self.block(child, width, None, quote));
                } else {
                    let text = self.inline(std::slice::from_ref(child), quote);
                    if !text.is_empty() {
                        item_rows.push(Row::new(text));
                    }
                }
            }
            if item_rows.is_empty() {
                let mut r = Row::new(first);
                r.wrapped = true;
                rows.push(r);
                continue;
            }
            for (row_index, row) in item_rows.into_iter().enumerate() {
                if row.nested {
                    rows.push(row);
                    continue;
                }
                let first_prefix = if row_index == 0 { &first } else { &continuation };
                if row.literal {
                    for l in render_utils::wrap(&row.text, width.max(1)) {
                        let mut r = Row::new(l);
                        r.literal = true;
                        r.wrapped = true;
                        rows.push(r);
                    }
                    continue;
                }
                let prefix_width = render_utils::width(first_prefix);
                if prefix_width >= width {
                    let mut r = Row::new(String::from_utf16_lossy(&super::text::truncate_to_width(
                        &first_prefix.encode_utf16().collect::<Vec<_>>(),
                        width.min(u32::MAX as usize) as u32,
                        Some(super::text::Ellipsis::Omit),
                        None,
                        3,
                    )));
                    r.wrapped = true;
                    rows.push(r);
                    for l in render_utils::wrap(&row.text, width.max(1)) {
                        let mut r = Row::new(l);
                        r.wrapped = true;
                        rows.push(r);
                    }
                    continue;
                }
                let wrapped = render_utils::wrap(&row.text, width - prefix_width);
                for (i, l) in wrapped.into_iter().enumerate() {
                    let mut r = Row::new(format!("{}{l}", if i == 0 { first_prefix } else { &continuation }));
                    r.wrapped = true;
                    rows.push(r);
                }
            }
        }
        rows
    }
    fn table(&self, token: &Token, width: usize, quote: bool) -> Vec<String> {
        let header =
            token.children.iter().find(|t| t.kind == Kind::TableHead).map(|h| h.children.as_slice()).unwrap_or(&[]);
        let body: Vec<_> = token.children.iter().filter(|t| t.kind == Kind::TableRow).collect();
        let n = header.len();
        if n == 0 {
            return vec![];
        }
        let overhead = 3 * n + 1;
        let available = width.saturating_sub(overhead);
        if width < overhead + n {
            return render_utils::wrap(&token.raw, width);
        }
        let cells = std::iter::once(header)
            .chain(body.iter().map(|r| r.children.as_slice()))
            .map(|row| row.iter().map(|c| self.inline(&c.children, quote)).collect::<Vec<_>>())
            .collect::<Vec<_>>();
        let mut natural = vec![0; n];
        let mut words = vec![1; n];
        for row in &cells {
            for (i, c) in row.iter().take(n).enumerate() {
                natural[i] = natural[i].max(c.split('\n').map(render_utils::width).max().unwrap_or(0));
                words[i] = words[i].max(c.split_whitespace().map(render_utils::width).max().unwrap_or(0).min(30));
            }
        }
        let mut minimum = words.clone();
        if minimum.iter().sum::<usize>() > available {
            minimum.fill(1);
            let remaining = available - n;
            let weight = words.iter().map(|w| w - 1).sum::<usize>();
            let growth = words
                .iter()
                .map(|w| {
                    if weight > 0 { (((w - 1) as f64 / weight as f64) * remaining as f64).floor() as usize } else { 0 }
                })
                .collect::<Vec<_>>();
            for i in 0..n {
                minimum[i] += growth[i];
            }
            let leftover = remaining - growth.iter().sum::<usize>();
            for value in minimum.iter_mut().take(leftover) {
                *value += 1;
            }
        }
        let mut widths = if natural.iter().sum::<usize>() + overhead <= width {
            natural.iter().zip(&minimum).map(|(a, b)| (*a).max(*b)).collect::<Vec<_>>()
        } else {
            let potential = natural.iter().zip(&minimum).map(|(a, b)| a.saturating_sub(*b)).sum::<usize>();
            let extra = available - minimum.iter().sum::<usize>();
            minimum
                .iter()
                .enumerate()
                .map(|(i, m)| {
                    m + if potential > 0 {
                        ((natural[i].saturating_sub(*m) as f64 / potential as f64) * extra as f64).floor() as usize
                    } else {
                        0
                    }
                })
                .collect::<Vec<_>>()
        };
        let mut remaining = available.saturating_sub(widths.iter().sum());
        while remaining > 0 {
            let mut grew = false;
            for i in 0..n {
                if remaining > 0 && widths[i] < natural[i] {
                    widths[i] += 1;
                    remaining -= 1;
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }
        let sym = |k: &str| self.theme.symbol(&format!("boxSharp.{k}")).to_owned();
        let h = sym("horizontal");
        let v = sym("vertical");
        let border = |left: &str, joint: &str, right: &str| {
            format!(
                "{}{h}{}{h}{}",
                sym(left),
                widths.iter().map(|w| h.repeat(*w)).collect::<Vec<_>>().join(&format!("{h}{}{h}", sym(joint))),
                sym(right)
            )
        };
        let separator = border("teeRight", "cross", "teeLeft");
        let mut rows = vec![border("topLeft", "teeDown", "topRight")];
        for (index, row) in std::iter::once(header).chain(body.iter().map(|r| r.children.as_slice())).enumerate() {
            let rendered = row.iter().map(|cell| self.inline(&cell.children, quote)).collect::<Vec<_>>();
            let wrapped = rendered
                .iter()
                .take(n)
                .enumerate()
                .map(|(i, c)| {
                    let mut lines = render_utils::wrap(c, widths[i].max(1));
                    while lines.len() > 1 && lines.last().is_some_and(|l| l.is_empty()) {
                        lines.pop();
                    }
                    lines.into_iter().map(|l| format!("{l}\x1b[22m\x1b[23m\x1b[39m")).collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            let height = wrapped.iter().map(Vec::len).max().unwrap_or(0);
            for line_index in 0..height {
                let mut parts = vec![];
                for (i, cell) in wrapped.iter().enumerate() {
                    let text = cell.get(line_index).cloned().unwrap_or_default();
                    let padded = format!("{text}{}", " ".repeat(widths[i].saturating_sub(render_utils::width(&text))));
                    parts.push(if index == 0 { self.attr(&padded, 1, 22) } else { padded });
                }
                rows.push(format!("{v} {} {v}", parts.join(&format!(" {v} "))));
            }
            if index + 1 < cells.len() {
                rows.push(separator.clone());
            }
        }
        if cells.len() == 1 {
            rows.push(separator);
        }
        rows.push(border("bottomLeft", "teeUp", "bottomRight"));
        rows
    }
    fn html_block(&self, source: &str, width: usize, quote: bool) -> Vec<Row> {
        let re = regex::Regex::new(r"(?is)<hr\b[^>]*/?>|<blockquote\b[^>]*>(.*?)</blockquote>").unwrap();
        let mut rows = vec![];
        let mut state = html::HtmlState::default();
        let code = |t: &str| format!("{}{}", self.fg("mdCode", t), self.default_prefix(quote));
        let add = |rows: &mut Vec<Row>, chunk: &str, state: &mut html::HtmlState| {
            let normalized = html::normalize(chunk, state, Some(&code));
            if !normalized.trim().is_empty() {
                for line in terminal_lines(&normalized) {
                    let line = line.trim_end();
                    rows.push(Row::new(if line.trim().is_empty() {
                        String::new()
                    } else {
                        self.default_text(line, quote)
                    }));
                }
            }
        };
        let mut last = 0;
        for caps in re.captures_iter(source) {
            let m = caps.get(0).unwrap();
            add(&mut rows, &source[last..m.start()], &mut state);
            last = m.end();
            if let Some(inner) = caps.get(1) {
                let normalized =
                    html::normalize(inner.as_str(), &mut html::HtmlState::default(), Some(&|t| self.fg("mdCode", t)));
                let mut lines =
                    terminal_lines(&normalized).into_iter().map(|l| l.trim_end().to_owned()).collect::<Vec<_>>();
                while lines.last().is_some_and(|l| l.is_empty()) {
                    lines.pop();
                }
                rows.extend(self.quote_border(lines.into_iter().map(Row::new).collect(), width));
            } else {
                rows.push(Row::new(self.hr(width, None)));
            }
        }
        add(&mut rows, &source[last..], &mut state);
        rows
    }
    fn swatch(&self, hex: &str, text: &str) -> Option<String> {
        let (fg, bg) = markdown_math::latex_color(None, &format!("#{hex}"), self.options.true_color)?;
        let hex = if hex.len() == 3 { hex.chars().flat_map(|c| [c, c]).collect::<String>() } else { hex.into() };
        let r = u8::from_str_radix(&hex[0..2], 16).ok()? as u32;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()? as u32;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()? as u32;
        let light = (r * 299 + g * 587 + b * 114) / 1000 >= 128;
        let contrast = if self.options.true_color {
            if light { "\x1b[38;2;0;0;0m" } else { "\x1b[38;2;255;255;255m" }
        } else if light {
            "\x1b[38;5;16m"
        } else {
            "\x1b[38;5;231m"
        };
        Some(format!("{fg}{}\x1b[39m {bg}{contrast}{text}\x1b[39m\x1b[49m", self.theme.symbol("md.colorSwatch")))
    }
    fn code_swatch(&self, source: &str) -> Option<String> {
        let hex = source.trim().strip_prefix('#')?;
        if !matches!(hex.len(), 3 | 6 | 8) || !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        self.swatch(hex, source.trim())
    }
    fn text_swatches(&self, source: &str, quote: bool) -> String {
        let re = regress::Regex::new(
            r"(?<![\w#&])#(?![0-9a-fA-F]{8}(?:-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12})([0-9a-fA-F]{3,8})(?!\w)",
        )
        .unwrap();
        let mut out = String::new();
        let mut last = 0;
        for m in re.find_iter(source) {
            let range = m.range();
            let hex = &source[range.start + 1..range.end];
            if !matches!(hex.len(), 3 | 6 | 8) || hex.len() == 3 && !hex.bytes().any(|c| c.is_ascii_alphabetic()) {
                continue;
            }
            let Some(swatch) = self.swatch(hex, &source[range.clone()]) else {
                continue;
            };
            out.push_str(&self.default_text(&source[last..range.start], quote));
            out.push_str(&swatch);
            last = range.end;
        }
        out.push_str(&self.default_text(&source[last..], quote));
        out
    }
}
impl StaticComponent for MarkdownComponent {
    fn render(&mut self, width: usize) -> Arc<Vec<String>> {
        self.render(width)
    }
    fn invalidate(&mut self, theme: Arc<Theme>) {
        self.invalidate(theme)
    }
    fn set_ignore_tight(&mut self, ignore: bool) {
        if self.ignore_tight != ignore {
            self.ignore_tight = ignore;
            self.cache = None;
        }
    }
}
fn split_highlight_lines(pushed: &str) -> (Vec<String>, String) {
    let mut lines = pushed.split('\n').map(str::to_owned).collect::<Vec<_>>();
    let pending = lines.pop().unwrap_or_default();
    (lines, pending)
}
fn code_has_closing_fence(raw: &str) -> bool {
    let mut lines = raw.split('\n');
    let Some(opening) = lines.next() else {
        return false;
    };
    let trimmed = opening.trim_start();
    if opening.len() - trimmed.len() > 3 {
        return false;
    }
    let Some(marker) = trimmed.chars().next().filter(|c| matches!(c, '`' | '~')) else {
        return false;
    };
    let length = trimmed.chars().take_while(|c| *c == marker).count();
    if length < 3 {
        return false;
    }
    lines.any(|line| {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        let closing = trimmed.chars().take_while(|c| *c == marker).count();
        indent <= 3 && closing >= length && trimmed[closing..].trim().is_empty()
    })
}
fn apply_background(line: &str, width: usize, bg: &StyleFunction) -> String {
    let mut padded = format!("{line}{}", " ".repeat(width.saturating_sub(render_utils::width(line))));
    if line.contains("\x1b[49m") {
        let probe = bg("\x01");
        if let Some(i) = probe.find('\x01').filter(|i| *i > 0) {
            padded = padded.replace("\x1b[49m", &format!("\x1b[49m{}", &probe[..i]));
        }
    }
    bg(&padded)
}
fn sized_heading(text: &str) -> String {
    let mut out = String::new();
    let mut ascii = String::new();
    let flush = |out: &mut String, ascii: &mut String| {
        if !ascii.is_empty() {
            out.push_str(&format!("\x1b]66;s=2;{ascii}\x1b\\"));
            ascii.clear();
        }
    };
    for g in text.graphemes(true) {
        if g.bytes().all(|c| (0x20..=0x7e).contains(&c)) {
            ascii.push_str(g);
        } else {
            flush(&mut out, &mut ascii);
            out.push_str(&format!(
                "\x1b]66;s=2:w={};{}\x1b\\",
                render_utils::width(g),
                g.replace(|c: char| c < ' ' || c == '\u{7f}', " ")
            ));
        }
    }
    flush(&mut out, &mut ascii);
    out
}
fn display_body(tokens: &[Token]) -> Option<String> {
    if let Some(math) = sole_display(tokens) {
        return Some(math.text.clone());
    }
    // Fixed bareMathEnvBlock folds one immediately preceding plain lhs/opening
    // delimiter line into the display body before the Markdown block splits.
    let math = tokens.last()?;
    if math.kind != (Kind::Math { display: true }) || !math.text.starts_with("\\begin{") {
        return None;
    }
    let prefix = &tokens[..tokens.len() - 1];
    if !prefix.iter().all(|t| matches!(t.kind, Kind::Text | Kind::SoftBreak)) {
        return None;
    }
    let plain = markdown_lexer::plain_inline(prefix);
    let lhs = plain.trim_end();
    if !lhs.contains('\n') && lhs.ends_with(['=', '(', '[', '{']) {
        Some(format!("{lhs}\n{}", math.text))
    } else {
        None
    }
}
fn sole_display(tokens: &[Token]) -> Option<&Token> {
    let mut significant =
        tokens.iter().filter(|t| !(matches!(t.kind, Kind::Text | Kind::SoftBreak) && t.text.trim().is_empty()));
    let t = significant.next()?;
    if significant.next().is_none() && t.kind == (Kind::Math { display: true }) { Some(t) } else { None }
}
fn style(text: &str, on: u8, off: u8) -> String {
    let open = format!("\x1b[{on}m");
    let close = format!("\x1b[{off}m");
    format!("{open}{}{close}", text.replace(&close, &format!("{close}{open}")))
}
fn terminal_lines(text: &str) -> Vec<&str> {
    let mut rows = text.split('\n').collect::<Vec<_>>();
    while rows.len() > 1 && rows.last() == Some(&"") {
        rows.pop();
    }
    rows
}
fn repair_orphan_fence(source: &str) -> String {
    let fence = regex::Regex::new(r"^ {0,3}(`{3,}|~{3,})[ \t]*(.*)$").unwrap();
    let mut lines = source.split('\n').collect::<Vec<_>>();
    let mut open: Option<(usize, String, String)> = None;
    for (i, line) in lines.iter().enumerate() {
        if let Some(c) = fence.captures(line) {
            let marker = c[1].to_owned();
            let info = c[2].trim().to_owned();
            if let Some((_, prior, _)) = &open {
                if marker.as_bytes()[0] == prior.as_bytes()[0] && marker.len() >= prior.len() && info.is_empty() {
                    open = None;
                }
            } else {
                open = Some((i, marker, info));
            }
        }
    }
    let Some((index, _, info)) = open else {
        return source.into();
    };
    if !info.is_empty() {
        return source.into();
    }
    let previous = lines[..index].iter().rev().map(|l| l.trim()).find(|l| !l.is_empty()).unwrap_or("");
    if previous.is_empty()
        || previous.ends_with(':')
        || regex::Regex::new(r"(?i)\b(?:code|example|markdown|output|snippet|source)\s*:?\s*$")
            .unwrap()
            .is_match(previous)
    {
        return source.into();
    }
    let heading = regex::Regex::new(r"^ {0,3}#{1,6}[ \t]+\S").unwrap();
    let delimiter = regex::Regex::new(r"^:?-{3,}:?$").unwrap();
    let mut has_heading = false;
    let mut has_table = false;
    for i in index + 1..lines.len() {
        has_heading |= heading.is_match(lines[i]);
        if lines[i].contains('|') && lines[i - 1].contains('|') {
            let split = |line: &str| {
                line.trim()
                    .trim_start_matches('|')
                    .trim_end_matches('|')
                    .split('|')
                    .map(|s| s.trim().to_owned())
                    .collect::<Vec<_>>()
            };
            let d = split(lines[i]);
            let h = split(lines[i - 1]);
            has_table |= d.len() >= 2
                && h.len() == d.len()
                && d.iter().all(|s| delimiter.is_match(s))
                && h.iter().all(|s| !s.is_empty());
        }
        if has_heading && has_table {
            lines.remove(index);
            return lines.join("\n");
        }
    }
    source.into()
}
fn tree_wrap(text: &str, width: usize) -> Option<Vec<String>> {
    let branch = regex::Regex::new(r"[├┣╠└┗╚╰][─━═]").unwrap();
    let sgr = regex::Regex::new(r"\x1b\[[0-9;:]*m").unwrap();
    let mut any = false;
    let mut out = vec![];
    let mut carry = String::new();
    for line in text.split('\n') {
        let mut codes = String::new();
        let mut guides = String::new();
        let mut end = 0;
        while end < line.len() {
            if let Some(m) = sgr.find_at(line, end)
                && m.start() == end
            {
                codes.push_str(m.as_str());
                end = m.end();
                continue;
            }
            let c = line[end..].chars().next().unwrap();
            if !matches!(c, '│' | '┃' | '║' | '├' | '┣' | '╠' | '└' | '┗' | '╚' | '╰' | '─' | '━' | '═' | ' ')
            {
                break;
            }
            guides.push(c);
            end += c.len_utf8();
        }
        let hangs = width >= 8
            && end < line.len()
            && branch.is_match(&guides)
            && render_utils::width(line) > width
            && width.saturating_sub(render_utils::width(&guides)) >= 8;
        if hangs {
            any = true;
            let active = format!("{carry}{codes}");
            let rows = render_utils::wrap(&format!("{active}{}", &line[end..]), width - render_utils::width(&guides));
            let mut hang = guides
                .chars()
                .map(|c| match c {
                    '│' | '┃' | '║' => c,
                    '├' => '│',
                    '┣' => '┃',
                    '╠' => '║',
                    _ => ' ',
                })
                .collect::<String>();
            hang.push_str(&" ".repeat(render_utils::width(&guides).saturating_sub(render_utils::width(&hang))));
            if let Some(first) = rows.first() {
                out.push(format!("{carry}{}{}", &line[..end], first.strip_prefix(&active).unwrap_or(first)));
            }
            for row in rows.iter().skip(1) {
                out.push(format!("{active}{hang}{row}"));
            }
        } else {
            out.push(format!("{carry}{line}"));
        }
        for m in sgr.find_iter(line) {
            carry.push_str(m.as_str());
        }
        let reset = carry.rfind("\x1b[0m").map(|p| p + 4).into_iter().chain(carry.rfind("\x1b[m").map(|p| p + 3)).max();
        if let Some(reset) = reset {
            carry = carry[reset..].into();
        }
    }
    any.then_some(out)
}

/// Fixed inline utility. Absence is the native equivalent of undefined/null;
/// base_color is also applied to its empty fallback, as in the original.
pub fn render_inline_markdown(
    text: Option<&WireString>,
    theme: Arc<Theme>,
    base_color: Option<&dyn Fn(&str) -> String>,
) -> String {
    render_inline_markdown_with_options(text, theme, base_color, MarkdownOptions::default())
}

/// Bind the same Markdown theme callbacks for the host's inline utility.
pub fn render_inline_markdown_with_options(
    text: Option<&WireString>,
    theme: Arc<Theme>,
    base_color: Option<&dyn Fn(&str) -> String>,
    options: MarkdownOptions,
) -> String {
    let identity = |s: &str| s.to_owned();
    let apply = base_color.unwrap_or(&identity);
    let Some(text) = text else {
        return apply("");
    };
    let mut c = MarkdownComponent::new(text.clone(), theme);
    c.set_options(options);
    let tokens = markdown_lexer::lex_document(&normalize_osc8_terminators(&String::from_utf16_lossy(text.units())));
    let mut result = String::new();
    for token in &tokens {
        match &token.kind {
            Kind::Math { .. } => {
                result.push_str(&apply(&markdown_math::latex_to_unicode(&token.text).replace('\n', " ")))
            }
            Kind::Paragraph => result.push_str(&inline_utility(&token.children, &c, apply)),
            Kind::List(start) => {
                let items: Vec<_> = token
                    .children
                    .iter()
                    .filter(|t| t.kind == Kind::Item)
                    .enumerate()
                    .map(|(i, item)| {
                        let prefix = start.map(|s| format!("{}. ", s.max(1) + i as u64)).unwrap_or_else(|| "• ".into());
                        format!("{}{}", apply(&prefix), inline_utility(&item.children, &c, apply))
                    })
                    .collect();
                result.push_str(&items.join(&apply(" ")));
            }
            _ => result.push_str(&apply(&html::entities(&token.text))),
        }
    }
    result
}
fn inline_utility(tokens: &[Token], c: &MarkdownComponent, apply: &dyn Fn(&str) -> String) -> String {
    let mut out = String::new();
    let reset = apply("");
    let mut state = html::HtmlState::default();
    let mut i = 0;
    while i < tokens.len() {
        let token = &tokens[i];
        let nested = || inline_utility(&token.children, c, apply);
        match &token.kind {
            Kind::Math { .. } => out.push_str(&apply(&markdown_math::latex_to_unicode(&token.text).replace('\n', " "))),
            Kind::Paragraph => out.push_str(&nested()),
            Kind::Text => {
                out.push_str(&if token.children.is_empty() { apply(&html::entities(&token.text)) } else { nested() })
            }
            Kind::Strong | Kind::Em | Kind::Del => {
                let (on, off) = match token.kind {
                    Kind::Strong => (1, 22),
                    Kind::Em => (3, 23),
                    _ => (9, 29),
                };
                out.push_str(&c.attr(&nested(), on, off));
                out.push_str(&reset);
            }
            Kind::Code => {
                out.push_str(&c.fg("mdCode", &token.text));
                out.push_str(&reset);
            }
            Kind::Link(_) => {
                out.push_str(&c.fg("mdLink", &c.attr(&nested(), 4, 24)));
                out.push_str(&reset);
            }
            Kind::Html => {
                if token.text.eq_ignore_ascii_case("<code>") {
                    if let Some(end) = tokens[i + 1..]
                        .iter()
                        .position(|t| t.kind == Kind::Html && t.text.eq_ignore_ascii_case("</code>"))
                    {
                        out.push_str(
                            &c.fg(
                                "mdCode",
                                &html::entities(&markdown_lexer::plain_inline(&tokens[i + 1..i + 1 + end])),
                            ),
                        );
                        out.push_str(&reset);
                        i += end + 2;
                        continue;
                    }
                }
                out.push_str(&apply(&html::normalize(&token.raw, &mut state, None)));
            }
            Kind::SoftBreak | Kind::Br => out.push_str(&apply("\n")),
            _ => out.push_str(&apply(&html::entities(&token.text))),
        }
        i += 1;
    }
    out
}
#[cfg(test)]
#[path = "markdown_capture_tests.rs"]
mod capture_tests;
#[cfg(test)]
#[path = "markdown_original_tests.rs"]
mod original_tests;
#[cfg(test)]
#[path = "markdown_stream_original_tests.rs"]
mod stream_original_tests;

#[cfg(test)]
#[path = "markdown_writer_original_emitter.rs"]
mod writer_original_emitter;

#[cfg(test)]
mod native_factory_resolution_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[test]
    fn present_factory_success_none_returns_without_diff_fallback() {
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let mut options = MarkdownOptions::default();
        options.highlighter = Some(Arc::new(move |code, _| {
            observed.fetch_add(1, Ordering::SeqCst);
            code.split('\n').map(str::to_owned).collect()
        }));
        options.highlight_stream_factory = Some(Arc::new(|_| Ok(None)));
        let mut component = MarkdownComponent::new("```diff\n+one\npartial".into(), Arc::new(Theme::dark()));
        component.set_options(options.clone());
        component.set_transient_render_cache(true);
        let _ = component.render(80);
        assert_eq!(calls.load(Ordering::SeqCst), 0, "successful present null factory cannot reach diff emulation");
        options.highlight_stream_factory = Some(Arc::new(|_| Err("native factory failure".into())));
        component.set_options(options);
        let _ = component.render(80);
        assert!(calls.load(Ordering::SeqCst) > 0, "error factory reaches fixed diff-family fallback");
    }
}
