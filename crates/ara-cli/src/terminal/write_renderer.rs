//! Fixed OMP596f2da WRITE display only; MIT, see THIRD_PARTY_NOTICES.md.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc.
use super::{
    graph_renderer::DisplayGraph,
    hyperlink::file_hyperlink,
    output_block::{self, State},
    render_utils::{self, sanitize},
    theme::{Theme, language_from_path},
    types::*,
};
use ara_rpc::{
    WireString,
    value_heap::{ValueError, ValueHandle, ValueSnapshot},
};
use std::sync::Arc;
type Result<T> = std::result::Result<T, ValueError>;

#[derive(Clone, Default)]
pub struct WriteArgs {
    pub path: Option<WireString>,
    pub path_present: bool,
    pub content_present:bool,
    pub content_is_string:bool,
    pub content: WireText,
}
#[derive(Clone, Default)]
pub struct WriteResult {
    pub text: WireText,
    pub resolved_path: Option<WireString>,
    pub made_executable: bool,
    pub diagnostics: Option<Diagnostics>,
    pub error: bool,
}
#[derive(Clone, Default)]
struct LineIndex {
    length: usize,
    suffix: Vec<u16>,
    lines: usize,
    initialized: bool,
    max_guard_units: usize,
}
impl LineIndex {
    fn count(&mut self, content: &WireString) -> usize {
        let units = content.units();
        if self.initialized {
            self.max_guard_units = self.max_guard_units.max(self.suffix.len());
        }
        let append = self.initialized
            && units.len() >= self.length
            && units.get(self.length - self.suffix.len()..self.length) == Some(self.suffix.as_slice());
        let start = if append { self.length } else { 0 };
        self.lines = if append { self.lines } else { 1 };
        self.lines += units[start..].iter().filter(|&&u| u == 10).count();
        self.length = units.len();
        self.suffix = units[units.len().saturating_sub(64)..].to_vec();
        self.initialized = true;
        self.lines
    }
}
#[derive(Clone)]
struct BodyCache {
    theme: (u64, u64),
    expanded: bool,
    revision: u64,
    result: bool,
    body: Vec<String>,
}
#[derive(Clone, Default)]
pub struct WriteComponent {
    pub args: WriteArgs,
    pub result: Option<WriteResult>,
    pub raw_block: Option<ValueHandle>,
    index: LineIndex,
    body: Option<BodyCache>,
    normalized: Option<(u64, WireString, usize)>,
    pub normalization_calls: usize,
    reveal: WriteReveal,
    pub highlight_calls: usize,
    pub revision: u64,
    pub first_result: bool,
}
impl WriteComponent {
    pub(crate) fn rebuild_live(
        &mut self,
        g: &DisplayGraph<'_>,
        args: &ValueHandle,
        now: std::time::Instant,
        smooth: bool,
    ) -> Result<()> {
        let raw = if let Some(block) = &self.raw_block {
            let key = ara_ai::graph_values::streaming_partial_json_key(g.0.heap())?;
            match g.0.heap().snapshot(&g.0.heap().get(block, &key, g.runtime())?)? {
                ValueSnapshot::String(raw) if !raw.is_empty() => Some(raw),
                _ => None,
            }
        } else {
            None
        };
        if let Some(raw) = raw {
            self.reveal.set_target(raw, now, smooth);
            if let Some(decoded) = self.reveal.decode(g, args)? {
                self.update_args(args_from_graph(g, &decoded)?);
            }
            Ok(())
        } else {
            self.reveal = WriteReveal::default();
            self.rebuild(g, args, None, false, true)
        }
    }
    pub(crate) fn reveal_deadline(&self) -> Option<std::time::Instant> {
        self.reveal.next_tick
    }
    pub(crate) fn advance_reveal(
        &mut self,
        g: &DisplayGraph<'_>,
        args: &ValueHandle,
        now: std::time::Instant,
    ) -> Result<()> {
        if self.reveal.advance(now) {
            if let Some(decoded) = self.reveal.decode(g, args)? {
                self.update_args(args_from_graph(g, &decoded)?);
            }
        }
        Ok(())
    }
    pub(crate) fn stop_reveal(&mut self) {
        // Fixed finish/stop deletes the reveal entry, releasing raw buffers
        // and parsed handles while the component retains its visible content.
        self.reveal = WriteReveal::default();
    }
    pub fn update_args(&mut self, args: WriteArgs) {
        if self.args.path == args.path
            && self.args.path_present == args.path_present
            && self.args.content_present == args.content_present
            && self.args.content_is_string == args.content_is_string
            && self.args.content.0 == args.content.0
        {
            return;
        }
        self.args = args;
        self.revision = self.revision.wrapping_add(1);
        self.body = None;
        self.normalized = None;
    }
    pub fn needs_first_result_repaint(&self, expanded: bool) -> bool {
        !self.first_result
            && !expanded
            && self.args.content.0.units().iter().filter(|&&u| u == 10).take(12).count() >= 12
    }
    #[cfg(test)]
    pub(super) fn reveal_observation(&self) -> (usize, usize) {
        (self.reveal.revealed, self.reveal.parse_calls)
    }
    #[cfg(test)]
    pub(super) fn max_append_guard_units(&self) -> usize {
        self.index.max_guard_units
    }
    pub fn update_result(&mut self, result: WriteResult, expanded: bool) -> bool {
        let repaint = self.needs_first_result_repaint(expanded);
        self.first_result = true;
        self.result = Some(result);
        self.revision = self.revision.wrapping_add(1);
        self.body = None;
        repaint
    }
    pub fn invalidate(&mut self) {
        self.body = None;
    }

    pub(crate) fn rebuild(
        &mut self,
        g: &DisplayGraph<'_>,
        args: &ValueHandle,
        result: Option<&ValueHandle>,
        error: bool,
        partial: bool,
    ) -> Result<()> {
        let decoded = if let Some(block) = &self.raw_block {
            let key = ara_ai::graph_values::streaming_partial_json_key(g.0.heap())?;
            let raw = g.0.heap().get(block, &key, g.runtime())?;
            match g.0.heap().snapshot(&raw)? {
                ValueSnapshot::String(raw) if !raw.is_empty() => decode_graph_args(g, args, &raw)?,
                _ => args.clone(),
            }
        } else {
            args.clone()
        };
        self.update_args(args_from_graph(g, &decoded)?);
        if let Some(result) = result {
            self.result = Some(result_from_graph(g, result, error, partial)?);
            self.first_result = true;
        }
        Ok(())
    }
    pub fn render(
        &mut self,
        width: usize,
        frame: &FrameContext,
        options: RenderOptions,
        theme: &Arc<Theme>,
    ) -> Vec<String> {
        if self.result.is_none() && !self.args.path_present {
            return Vec::new();
        }
        let raw_path = self.args.path.as_ref().map(sanitize).unwrap_or_default();
        // Device renderer is supplied by the separate xdev lane. Never paint
        // a local-file success while that dispatch owns the path.
        if self.result.is_none() && self.args.path.as_ref().is_some_and(super::xdev_renderer::could_be_device) {
            return Vec::new();
        }
        let path = render_utils::shorten_path(&raw_path);
        let language = language_from_path(&raw_path);
        let mut path_label = if path.is_empty() { theme.fg("toolOutput", "…") } else { theme.fg("accent", &path) };
        if let Some(target) = self.result.as_ref().and_then(|r| r.resolved_path.as_ref()) {
            path_label = file_hyperlink(&sanitize(target), &path_label, None, frame.hyperlinks);
        }
        let text = &self.args.content.0;
        let full = self.result.is_some() || options.expanded;
        if full && self.normalized.as_ref().is_none_or(|(revision, _, _)| *revision != self.revision) {
            let normalized = WireString::from_units(text.units().iter().copied().filter(|&u| u != 13).collect());
            let total = normalized.units().iter().filter(|&&u| u == 10).count() + 1;
            self.normalized = Some((self.revision, normalized, total));
            self.normalization_calls += 1;
        }
        let (normalized, total) = if full {
            let (_, text, total) = self.normalized.as_ref().unwrap();
            (text, *total)
        } else {
            (text, self.index.count(text))
        };
        let result = self.result.as_ref();
        let error = result.is_some_and(|r| r.error);
        let partial = result.is_some() && options.is_partial;
        let line_suffix = if !error && result.is_some() && !normalized.is_empty() {
            theme.fg("dim", &format!(" · {total} line{}", if total == 1 { "" } else { "s" }))
        } else {
            String::new()
        };
        let executable = if !error && !partial && result.is_some_and(|r| r.made_executable) {
            format!("{}{}", theme.fg("dim", " · "), theme.fg("success", "made executable!"))
        } else {
            String::new()
        };
        let description =
            format!("{} {path_label}{line_suffix}{executable}", theme.fg("muted", theme.language_icon(language)));
        let icon = if error {
            Some(theme.status("error", None))
        } else if partial {
            Some(theme.status("running", options.spinner_frame))
        } else if result.is_some() {
            Some(theme.styled_symbol("tool.write", "accent"))
        } else {
            None
        };
        let header = output_block::status_line("Write", &description, icon.as_deref(), theme);
        if error {
            let body = result.map(|r| sanitize(&r.text.0)).unwrap_or_default();
            return output_block::framed(
                &header,
                &body.lines().map(|s| theme.fg("error", s)).collect::<Vec<_>>(),
                width,
                State::Error,
                theme,
            );
        }
        let hit = self.body.as_ref().is_some_and(|b| {
            b.theme == (theme.identity, theme.revision)
                && b.expanded == options.expanded
                && b.revision == self.revision
                && b.result == result.is_some()
        });
        if !hit {
            let start = if result.is_none() && !options.expanded { total.saturating_sub(12) } else { 0 };
            let offset = if start > 0 {
                let mut seen = 0;
                normalized
                    .units()
                    .iter()
                    .enumerate()
                    .rev()
                    .find_map(|(i, &u)| {
                        if u == 10 {
                            seen += 1;
                        }
                        if seen == 12 { Some(i + 1) } else { None }
                    })
                    .unwrap_or(0)
            } else {
                0
            };
            let visible = WireString::from_units(
                normalized.units()[offset..].iter().copied().filter(|&u| u != 13).collect::<Vec<_>>(),
            );
            let visible = sanitize(&visible);
            let mut lines = Vec::new();
            if !visible.is_empty() {
                if start > 0 {
                    lines.push(
                        theme.fg("dim", &format!("… ({start} earlier line{})", if start == 1 { "" } else { "s" })),
                    );
                }
                let preview = if result.is_some() && !options.expanded {
                    visible.split('\n').take(6).collect::<Vec<_>>().join("\n")
                } else {
                    visible
                };
                self.highlight_calls += 1;
                let highlighted = theme.highlight(&preview, language);
                let digits = 3usize.max(total.to_string().len());
                for (i, row) in highlighted.into_iter().enumerate() {
                    lines.push(format!(
                        "{}{}",
                        theme.fg("dim", &format!("{:>digits$} ", start + i + 1)),
                        render_utils::replace_tabs(&row)
                    ));
                }
                if result.is_some() && !options.expanded && total > 6 {
                    lines.push(
                        theme.fg("dim", &format!("… {} more lines {}", total - 6, render_utils::expand_hint(theme))),
                    );
                }
            }
            self.body = Some(BodyCache {
                theme: (theme.identity, theme.revision),
                expanded: options.expanded,
                revision: self.revision,
                result: result.is_some(),
                body: lines,
            });
        }
        let mut lines = self.body.as_ref().unwrap().body.clone();
        if result.is_none() && !lines.is_empty() {
            let spinner =
                options.spinner_frame.map(|s| format!("{} ", theme.status("running", Some(s)))).unwrap_or_default();
            lines.push(format!("{spinner}{}", theme.fg("dim", "… (streaming)")));
        }
        if let Some(result) = result {
            if partial && !result.text.0.is_empty() {
                lines.insert(
                    0,
                    theme.fg(
                        "muted",
                        &render_utils::truncate(&render_utils::replace_tabs(&sanitize(&result.text.0)), 110),
                    ),
                );
            }
            if !partial {
                if let Some(d) = &result.diagnostics {
                    let text = render_utils::diagnostics(d, options.expanded, theme);
                    lines.extend(text.lines().map(str::to_owned));
                }
            }
        }
        output_block::framed(
            &header,
            &lines,
            width,
            if result.is_some() && !partial { State::Success } else { State::Pending },
            theme,
        )
    }
}
fn graph_string(g: &DisplayGraph<'_>, v: &ValueHandle) -> Result<Option<WireString>> {
    Ok(match g.0.heap().snapshot(v)? {
        ValueSnapshot::String(s) => Some(s),
        _ => None,
    })
}
pub(crate) fn args_from_graph(g: &DisplayGraph<'_>, root: &ValueHandle) -> Result<WriteArgs> {
    let file = g.get(root, "file_path")?;
    let file_string = graph_string(g, &file)?;
    let path = if file_string.is_none() { g.get(root, "path")? } else { g.0.heap().undefined()? };
    let content = g.get(root, "content")?;
    let content_present=!matches!(g.0.heap().snapshot(&content)?,ValueSnapshot::Undefined);
    let content_is_string=matches!(g.0.heap().snapshot(&content)?,ValueSnapshot::String(_));
    let selected_path=file_string.as_ref().or(graph_string(g,&path)?.as_ref()).cloned();
    let is_device=selected_path.as_ref().is_some_and(|path| super::xdev_renderer::could_be_device(path));
    // xdev.decodeInnerArgs consumes only an original string. Nonstring payloads
    // must not invoke user coercion while the queued device card is built.
    let content = if g.nullish(&content)? || (is_device&&!content_is_string) { "".into() } else { g.0.heap().to_string(&content, g.runtime())? };
    Ok(WriteArgs {
        path: file_string.or(graph_string(g, &path)?),
        path_present: !matches!(g.0.heap().snapshot(&file)?, ValueSnapshot::Undefined)
            || !matches!(g.0.heap().snapshot(&path)?, ValueSnapshot::Undefined),
        content_present,
        content_is_string,
        content: WireText(content),
    })
}
pub(crate) fn decode_graph_args(g: &DisplayGraph<'_>, full: &ValueHandle, raw: &WireString) -> Result<ValueHandle> {
    let parsed = parse_graph_args(g, raw)?;
    let decoded = g.0.heap().spread(full, g.runtime())?;
    g.0.heap().spread_into(&decoded, &parsed, g.runtime())?;
    for (key, value) in streamed_strings(raw) {
        g.0.heap().define_own_data(&decoded, key.into(), g.0.heap().string(value)?, g.runtime())?;
    }
    g.0.heap().define_own_data(&decoded, "__partialJson".into(), g.0.heap().string(raw.clone())?, g.runtime())?;
    Ok(decoded)
}
pub(crate) fn parse_graph_args(g: &DisplayGraph<'_>, raw: &WireString) -> Result<ValueHandle> {
    let factory = g.0.factory().invocation(g.runtime());
    let parsed = ara_ai::relaxed_json::parse_streaming_json_units(raw, &factory).map_err(|error| match error {
        ara_ai::relaxed_json::FinalParseError::Runtime(e) => e,
        ara_ai::relaxed_json::FinalParseError::Syntax(e) => ValueError::TypeError(e.to_string()),
    })?;
    Ok(parsed)
}
#[derive(Clone, Copy, Default)]
enum ExtractState {
    #[default]
    Scan,
    Candidate,
    AfterCandidate,
    BeforeValue,
    Target,
}
/// Fixed tool-args-reveal.ts:81-297; WRITE extracts content only. Preserve
/// UTF16, malformed escapes and duplicate-field transitions across prefixes.
#[derive(Clone)]
struct StreamingStringExtractor {
    source: WireString,
    offset: usize,
    depth: i32,
    state: ExtractState,
    candidate: Vec<u16>,
    escaped: bool,
    unicode: Vec<u16>,
    matched: bool,
    target: bool,
    value: Option<Vec<u16>>,
}
impl Default for StreamingStringExtractor {
    fn default() -> Self {
        Self {
            source: WireString::from(""),
            offset: 0,
            depth: 0,
            state: ExtractState::default(),
            candidate: Vec::new(),
            escaped: false,
            unicode: Vec::new(),
            matched: false,
            target: false,
            value: None,
        }
    }
}
impl StreamingStringExtractor {
    fn update(&mut self, raw: &WireString) -> bool {
        if !raw.units().starts_with(self.source.units()) {
            *self = Self::default();
        }
        self.source = raw.clone();
        let mut changed = false;
        while let Some(&ch) = raw.units().get(self.offset) {
            match self.state {
                ExtractState::Scan => {
                    if ch == 34 {
                        self.candidate.clear();
                        self.escaped = false;
                        self.unicode.clear();
                        self.state = ExtractState::Candidate;
                    } else if matches!(ch, 123 | 91) {
                        self.depth += 1;
                    } else if matches!(ch, 125 | 93) {
                        self.depth -= 1;
                    }
                    self.offset += 1;
                }
                ExtractState::Candidate | ExtractState::Target => {
                    let target = matches!(self.state, ExtractState::Target);
                    if !self.unicode.is_empty() {
                        if hex_digit(ch).is_some() {
                            self.unicode.push(ch);
                            if self.unicode.len() == 5 {
                                let code = self.unicode[1..].iter().fold(0u16, |n, &u| n * 16 + hex_digit(u).unwrap());
                                self.append(target, &[code], &mut changed);
                                self.unicode.clear();
                                self.escaped = false;
                            }
                        } else {
                            let mut text = std::mem::take(&mut self.unicode);
                            text.push(ch);
                            self.append(target, &text, &mut changed);
                            self.escaped = false;
                        }
                    } else if self.escaped {
                        if ch == 117 {
                            self.unicode.push(ch);
                        } else {
                            self.append(target, &[decode_escape(ch)], &mut changed);
                            self.escaped = false;
                        }
                    } else if ch == 92 {
                        self.escaped = true;
                    } else if ch == 34 {
                        if target {
                            self.target = false;
                            self.state = ExtractState::Scan;
                        } else {
                            self.matched =
                                self.depth == 1 && self.candidate.as_slice() == [99, 111, 110, 116, 101, 110, 116];
                            self.state = ExtractState::AfterCandidate;
                        }
                    } else {
                        self.append(target, &[ch], &mut changed);
                    }
                    self.offset += 1;
                }
                ExtractState::AfterCandidate => {
                    if js_whitespace(ch) {
                        self.offset += 1;
                        continue;
                    }
                    let matched = std::mem::take(&mut self.matched);
                    if ch == 58 && matched {
                        self.target = true;
                        self.state = ExtractState::BeforeValue;
                        self.offset += 1;
                    } else {
                        self.state = ExtractState::Scan;
                    }
                }
                ExtractState::BeforeValue => {
                    if js_whitespace(ch) {
                        self.offset += 1;
                        continue;
                    }
                    if ch == 34 && self.target {
                        if self.value.as_ref().is_some_and(|v| !v.is_empty()) {
                            self.value = Some(Vec::new());
                            changed = true;
                        }
                        self.escaped = false;
                        self.unicode.clear();
                        self.state = ExtractState::Target;
                        self.offset += 1;
                    } else {
                        self.target = false;
                        self.state = ExtractState::Scan;
                    }
                }
            }
        }
        changed
    }
    fn append(&mut self, target: bool, text: &[u16], changed: &mut bool) {
        if target {
            if !self.target || text.is_empty() {
                return;
            }
            self.value.get_or_insert_with(Vec::new).extend_from_slice(text);
            *changed = true;
        } else {
            self.candidate.extend_from_slice(text);
        }
    }
    fn value(&self) -> Option<WireString> {
        self.value.as_ref().map(|v| WireString::from_units(v.clone()))
    }
}
fn hex_digit(ch: u16) -> Option<u16> {
    match ch {
        48..=57 => Some(ch - 48),
        65..=70 => Some(ch - 65 + 10),
        97..=102 => Some(ch - 97 + 10),
        _ => None,
    }
}
fn js_whitespace(ch: u16) -> bool {
    char::from_u32(ch as u32).is_some_and(ara_prompt::js::is_space)
}
fn decode_escape(ch: u16) -> u16 {
    match ch {
        98 => 8,
        102 => 12,
        110 => 10,
        114 => 13,
        116 => 9,
        _ => ch,
    }
}
fn streamed_strings(raw: &WireString) -> Vec<(&'static str, WireString)> {
    let mut extractor = StreamingStringExtractor::default();
    extractor.update(raw);
    extractor.value().into_iter().map(|value| ("content", value)).collect()
}
const REVEAL_FRAME: std::time::Duration = std::time::Duration::from_nanos(1_000_000_000 / 30);
/// WRITE's portion of the fixed tool-args reveal controller. The existing
/// evaluator owns ticks; no OS producer reads a heap or writes terminal rows.
#[derive(Clone)]
struct WriteReveal {
    target: WireString,
    revealed: usize,
    initialized: bool,
    next_tick: Option<std::time::Instant>,
    parsed: Option<ValueHandle>,
    parsed_len: usize,
    extractor: StreamingStringExtractor,
    display: Option<ValueHandle>,
    full: Option<ValueHandle>,
    parse_calls: usize,
}
impl Default for WriteReveal {
    fn default() -> Self {
        Self {
            target: WireString::from(""),
            revealed: 0,
            initialized: false,
            next_tick: None,
            parsed: None,
            parsed_len: 0,
            extractor: StreamingStringExtractor::default(),
            display: None,
            full: None,
            parse_calls: 0,
        }
    }
}
impl WriteReveal {
    fn set_target(&mut self, raw: WireString, now: std::time::Instant, smooth: bool) {
        if !self.initialized {
            self.initialized = true;
            self.revealed = raw.len();
        } else if !raw.units().starts_with(self.target.units()) {
            self.revealed = self.revealed.min(raw.len());
            self.parsed = None;
            self.parsed_len = 0;
            self.extractor = StreamingStringExtractor::default();
            self.display = None;
            self.full = None;
        }
        self.target = raw;
        if !smooth {
            self.revealed = self.target.len();
        }
        self.revealed = clamp_slice_end(&self.target, self.revealed);
        if self.revealed < self.target.len() {
            if self.next_tick.is_none() {
                self.next_tick = Some(now + REVEAL_FRAME);
            }
        } else {
            self.next_tick = None;
        }
    }
    fn advance(&mut self, now: std::time::Instant) -> bool {
        if self.next_tick.is_none_or(|deadline| now < deadline) {
            return false;
        }
        let backlog = self.target.len().saturating_sub(self.revealed);
        if backlog == 0 {
            self.next_tick = None;
            return false;
        }
        let step = 3usize.max(backlog.div_ceil(8));
        self.revealed = clamp_slice_end(&self.target, self.revealed.saturating_add(step));
        self.next_tick = (self.revealed < self.target.len()).then_some(now + REVEAL_FRAME);
        true
    }
    fn decode(&mut self, g: &DisplayGraph<'_>, full: &ValueHandle) -> Result<Option<ValueHandle>> {
        let prefix = WireString::from_units(self.target.units()[..self.revealed].to_vec());
        // json-parse.ts uses JS signed >>5, with the minimum floor 256.
        let growth = 256usize.max(((prefix.len() as u32 as i32) >> 5).max(0) as usize);
        let parse = prefix.len() > 0
            && (prefix.len() < 256 || self.parsed_len == 0 || prefix.len().saturating_sub(self.parsed_len) >= growth);
        let mut changed = false;
        if parse {
            self.parsed = Some(parse_graph_args(g, &prefix)?);
            self.parsed_len = prefix.len();
            self.parse_calls += 1;
            changed = true;
        }
        if self.extractor.update(&prefix) {
            let parsed = if let Some(parsed) = &self.parsed {
                g.0.heap().spread(parsed, g.runtime())?
            } else {
                g.0.heap().object()?
            };
            if let Some(value) = self.extractor.value() {
                g.0.heap().define_own_data(&parsed, "content".into(), g.0.heap().string(value)?, g.runtime())?;
            }
            self.parsed = Some(parsed);
            changed = true;
        }
        changed |= self.full.as_ref().is_none_or(|old| !old.same_identity(full));
        if !changed {
            return Ok(None);
        }
        let decoded = g.0.heap().spread(full, g.runtime())?;
        if let Some(parsed) = &self.parsed {
            g.0.heap().spread_into(&decoded, parsed, g.runtime())?;
        }
        g.0.heap().define_own_data(&decoded, "__partialJson".into(), g.0.heap().string(prefix)?, g.runtime())?;
        self.full = Some(full.clone());
        self.display = Some(decoded.clone());
        Ok(Some(decoded))
    }
}
fn clamp_slice_end(text: &WireString, end: usize) -> usize {
    let end = end.min(text.len());
    if end > 0 && end < text.len() && (0xd800..=0xdbff).contains(&text.units()[end - 1]) { end + 1 } else { end }
}
pub(crate) fn result_from_graph(
    g: &DisplayGraph<'_>,
    root: &ValueHandle,
    error: bool,
    partial: bool,
) -> Result<WriteResult> {
    let text =
        super::event_controller::content_from_graph(g, root, true)?.into_iter().next().unwrap_or_else(|| "".into());
    let details = g.get(root, "details")?;
    let mut result = WriteResult { text: WireText(text), error, ..Default::default() };
    if !g.nullish(&details)? {
        result.resolved_path = graph_string(g, &g.get(&details, "resolvedPath")?)?;
        if !partial {
            result.made_executable = g.truth(&g.get(&details, "madeExecutable")?)?;
            let diag = g.get(&details, "diagnostics")?;
            if !g.nullish(&diag)? {
                result.diagnostics = Some(super::event_controller::diagnostics_from_graph(g, &diag)?);
            }
        }
    }
    Ok(result)
}
pub(crate) fn args_from_json(v: &serde_json::Value) -> WriteArgs {
    fn string(v: &serde_json::Value) -> String {
        match v {
            serde_json::Value::Null => String::new(),
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Array(a) => a.iter().map(string).collect::<Vec<_>>().join(","),
            serde_json::Value::Object(_) => "[object Object]".into(),
            _ => v.to_string(),
        }
    }
    WriteArgs {
        content_present:v.get("content").is_some(),
        content_is_string:v.get("content").is_some_and(serde_json::Value::is_string),
        path: v
            .get("file_path")
            .and_then(serde_json::Value::as_str)
            .or_else(|| v.get("path").and_then(serde_json::Value::as_str))
            .map(Into::into),
        path_present: v.get("file_path").is_some() || v.get("path").is_some(),
        content: WireText(string(&v["content"]).into()),
    }
}
pub(crate) fn result_from_output(v: &ara_agent::ToolOutput) -> WriteResult {
    let d = v.details.as_ref();
    WriteResult {
        text: WireText(
            v.content
                .iter()
                .find_map(|b| match b {
                    ara_ai::UserBlock::Text(t) => Some(t.text.as_str().into()),
                    _ => None,
                })
                .unwrap_or_else(|| "".into()),
        ),
        error: v.is_error,
        resolved_path: d.and_then(|d| d.get("resolvedPath")).and_then(serde_json::Value::as_str).map(Into::into),
        made_executable: d.and_then(|d| d.get("madeExecutable")).and_then(serde_json::Value::as_bool).unwrap_or(false),
        diagnostics: d.and_then(|d| d.get("diagnostics")).map(|d| Diagnostics {
            errored: d["errored"].as_bool().unwrap_or(false),
            summary: WireText(d["summary"].as_str().unwrap_or("").into()),
            messages: d["messages"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
                .map(Into::into)
                .collect(),
        }),
    }
}
