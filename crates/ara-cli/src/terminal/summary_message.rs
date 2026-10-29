//! Fixed OMP 596f2da compaction-summary-message.ts (complete static component).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::{
    box_component::BoxComponent,
    markdown::{MarkdownComponent, MarkdownOptions},
    render_utils,
    theme::Theme,
    types::FrameContext,
};
use ara_rpc::{WireString, WireValue};
use std::{io, sync::Arc};

#[derive(Clone, Copy)]
enum SummaryKind {
    Compaction,
    Handoff,
    Branch,
}

pub struct SummaryMessageComponent {
    source: WireValue,
    kind: SummaryKind,
    theme: Arc<Theme>,
    expanded: bool,
    cache: Option<(usize, Arc<Vec<String>>)>,
    detail: Option<BoxComponent>,
    markdown_options: MarkdownOptions,
}
impl SummaryMessageComponent {
    pub fn from_wire(body: &WireValue, theme: Arc<Theme>) -> io::Result<Option<Self>> {
        Self::from_wire_with_options(body, theme, MarkdownOptions::default())
    }
    pub fn from_wire_with_options(
        body: &WireValue,
        theme: Arc<Theme>,
        markdown_options: MarkdownOptions,
    ) -> io::Result<Option<Self>> {
        let eq = |key: &str, value: &str| {
            body.get(key).and_then(WireValue::as_string).is_some_and(|s| s.equals_ascii(value))
        };
        let kind = if eq("role", "compactionSummary") {
            SummaryKind::Compaction
        } else if eq("role", "branchSummary") {
            SummaryKind::Branch
        } else if eq("role", "custom")
            && eq("customType", "handoff")
            && matches!(body.get("display"), Some(WireValue::Bool(true)))
        {
            SummaryKind::Handoff
        } else {
            return Ok(None);
        };
        Ok(Some(Self {
            source: body.clone(),
            kind,
            theme,
            expanded: false,
            cache: None,
            detail: None,
            markdown_options,
        }))
    }
    /// Transaction-local cache copy. Box children are rebuilt only if the
    /// retained width/capability cache misses; no live Graph read is performed.
    pub(crate) fn frame_copy(&self) -> Self {
        Self {
            source: self.source.clone(),
            kind: self.kind,
            theme: self.theme.clone(),
            expanded: self.expanded,
            cache: self.cache.clone(),
            detail: None,
            markdown_options: self.markdown_options.clone(),
        }
    }
    /// Current snapshot facts are the only runtime capability source. Preserve
    /// callbacks, default styles and source identity when capabilities change.
    pub fn set_frame_capabilities(&mut self, frame: &FrameContext) -> bool {
        if !self.markdown_options.set_frame_capabilities(frame) {
            return false;
        }
        self.cache = None;
        self.detail = None;
        true
    }
    /// Exact full owned Wire observation, retaining its original UTF16 units.
    pub fn source(&self) -> &WireValue {
        &self.source
    }
    pub fn set_expanded(&mut self, expanded: bool) {
        if self.expanded == expanded {
            return;
        }
        self.expanded = expanded;
        self.cache = None;
    }
    pub(crate) fn set_markdown_options(&mut self, options: MarkdownOptions) {
        self.markdown_options = options;
        self.cache = None;
        self.detail = None;
    }
    pub fn invalidate(&mut self, theme: Arc<Theme>) {
        self.theme = theme;
        self.cache = None;
        self.detail = None;
    }
    fn text(&self, key: &str) -> String {
        self.source
            .get(key)
            .and_then(WireValue::as_string)
            .map(|s| String::from_utf16_lossy(s.units()))
            .unwrap_or_default()
    }
    fn label(&self) -> String {
        match self.kind {
            SummaryKind::Handoff => format!("{} handed-off", self.theme.symbol("icon.context")),
            SummaryKind::Branch => format!("{} branch", self.theme.symbol("icon.branch")),
            SummaryKind::Compaction => {
                let method = self.text("method");
                let name = match method.as_str() {
                    "remote" => "remote-compacted",
                    "soft" => "soft-compacted",
                    "handoff" => "handed-off",
                    "snapcompact" => "snap-compacted",
                    "shake" => "shaken",
                    _ => "compacted",
                };
                let mut label = format!("{} {name}", self.theme.symbol("icon.camera"));
                let before = self.source.get("tokensBefore").and_then(WireValue::as_number).unwrap_or(0.0);
                if before > 0.0
                    && let Some(after) = self.source.get("tokensAfter").and_then(WireValue::as_number)
                {
                    label.push_str(&format!(
                        "{}{}→{}",
                        self.theme.symbol("sep.dot"),
                        format_number(before),
                        format_number(after)
                    ));
                }
                if !self.text("warning").is_empty() {
                    label.push_str(&format!(" {}", self.theme.fg("warning", self.theme.symbol("icon.warning"))));
                }
                label
            }
        }
    }
    fn divider(&self, width: usize) -> String {
        let label = self.label();
        let hint = format!("{} ctrl+o", self.theme.symbol("sep.dot").trim());
        let plain = render_utils::width(&format!("{label} {hint}"));
        if width < plain + 6 {
            return self.theme.fg("muted", &label);
        }
        let remaining = width - plain - 2;
        let left = remaining / 2;
        let right = remaining - left;
        format!(
            "{} {} {} {}",
            self.theme.fg("dim", &self.theme.symbol("tree.horizontal").repeat(left)),
            self.theme.fg("muted", &label),
            self.theme.fg("dim", &hint),
            self.theme.fg("dim", &self.theme.symbol("tree.horizontal").repeat(right))
        )
    }
    fn detail_markdown(&self) -> String {
        match self.kind {
            SummaryKind::Branch => format!("**Branch summary**\n\n{}", self.text("summary")),
            SummaryKind::Handoff => {
                let text = match self.source.get("content") {
                    Some(WireValue::String(s)) => String::from_utf16_lossy(s.units()),
                    Some(WireValue::Array(blocks)) => blocks
                        .iter()
                        .filter(|block| {
                            block.get("type").and_then(WireValue::as_string).is_some_and(|t| t.equals_ascii("text"))
                        })
                        .filter_map(|block| block.get("text").and_then(WireValue::as_string))
                        .map(|s| String::from_utf16_lossy(s.units()))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    _ => String::new(),
                };
                let document = extract_handoff_document(&text);
                format!(
                    "**Handoff context**\n\n{}",
                    if document.is_empty() { "_No handoff content._" } else { document }
                )
            }
            SummaryKind::Compaction => {
                let before = self.source.get("tokensBefore").and_then(WireValue::as_number).unwrap_or(0.0);
                let after = self.source.get("tokensAfter").and_then(WireValue::as_number);
                let line = match (before > 0.0, after) {
                    (true, Some(after)) => {
                        format!("Compacted from {} to {} tokens", locale_number(before), locale_number(after))
                    }
                    (true, None) => format!("Compacted from {} tokens", locale_number(before)),
                    (false, Some(after)) => format!("Compacted to {} tokens", locale_number(after)),
                    (false, None) => "Compacted context".into(),
                };
                let warning = self.text("warning");
                let warning = if warning.is_empty() {
                    String::new()
                } else {
                    format!("\n\n{} **Warning:** {warning}", self.theme.symbol("icon.warning"))
                };
                let count = self.source.get("images").and_then(WireValue::as_array).map_or(0, |v| v.len());
                let frames = if count > 0 {
                    format!("\n\n_{count} snapcompact frame{} attached_", if count == 1 { "" } else { "s" })
                } else {
                    String::new()
                };
                format!("**{line}**{warning}\n\n{}{frames}", self.text("summary"))
            }
        }
    }
    pub fn render(&mut self, width: usize) -> Arc<Vec<String>> {
        let width = width.max(1);
        if let Some((cached, rows)) = &self.cache
            && *cached == width
        {
            return rows.clone();
        }
        let mut rows = vec![String::new(), self.divider(width), String::new()];
        if self.expanded {
            if self.detail.is_none() {
                let mut detail = BoxComponent::new(1, 1, Some("customMessageBg".into()), None, self.theme.clone());
                detail.set_ignore_tight(true);
                let mut markdown = MarkdownComponent::new_with_color(
                    WireString::from(self.detail_markdown()),
                    self.theme.clone(),
                    "customMessageText",
                );
                markdown.set_options(self.markdown_options.clone());
                detail.add_child(Box::new(markdown));
                self.detail = Some(detail);
            }
            rows.extend(self.detail.as_mut().unwrap().render(width).iter().cloned());
        }
        let rows = Arc::new(rows);
        self.cache = Some((width, rows.clone()));
        rows
    }
}
fn extract_handoff_document(text: &str) -> &str {
    let Some(start) = text.find("<handoff-context>") else {
        return text.trim();
    };
    let rest = &text[start + "<handoff-context>".len()..];
    rest.find("</handoff-context>").map_or(rest, |end| &rest[..end]).trim()
}
fn format_number(n: f64) -> String {
    let trim = |n: f64| {
        let s = format!("{n:.1}");
        s.strip_suffix(".0").unwrap_or(&s).to_owned()
    };
    if n < 1_000.0 {
        return n.to_string();
    }
    if n < 10_000.0 {
        return format!("{}K", trim(n / 1_000.0));
    }
    if n < 1_000_000.0 {
        return format!("{}K", (n / 1_000.0).round());
    }
    if n < 10_000_000.0 {
        return format!("{}M", trim(n / 1_000_000.0));
    }
    if n < 1_000_000_000.0 {
        return format!("{}M", (n / 1_000_000.0).round());
    }
    if n < 10_000_000_000.0 {
        return format!("{}B", trim(n / 1_000_000_000.0));
    }
    format!("{}B", (n / 1_000_000_000.0).round())
}
fn locale_number(n: f64) -> String {
    let source = if n.fract() == 0.0 { format!("{n:.0}") } else { format!("{n:.3}").trim_end_matches('0').to_owned() };
    let source = source.trim_end_matches('.');
    let (sign, unsigned) = source.strip_prefix('-').map_or(("", source), |s| ("-", s));
    let (int, fraction) = unsigned.split_once('.').map_or((unsigned, None), |(a, b)| (a, Some(b)));
    let mut out = sign.to_owned();
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    if let Some(fraction) = fraction {
        out.push('.');
        out.push_str(fraction);
    }
    out
}

#[cfg(test)]
#[path = "summary_original_tests.rs"]
mod original_tests;
