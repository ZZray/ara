//! Fixed OMP palette/symbol and native highlighter adapter.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::highlight::{self, Color, HighlightColors};
use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT_THEME: AtomicU64 = AtomicU64::new(1);
#[derive(Clone, Debug)]
pub struct Theme {
    #[cfg(test)]
    pub stats_colors: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    pub identity: u64,
    pub revision: u64,
    fg: HashMap<String, String>,
    bg: HashMap<String, String>,
    symbols: HashMap<String, String>,
    pub spinner_frames: Vec<String>,
}
impl Theme {
    pub fn dark() -> Self {
        Self::from_json(include_str!("themes/dark.json"), "unicode").expect("fixed dark palette")
    }
    pub fn light() -> Self {
        Self::from_json(include_str!("themes/light.json"), "unicode").expect("fixed light palette")
    }
    pub fn from_json(source: &str, preset: &str) -> Result<Self, serde_json::Error> {
        let body: serde_json::Value = serde_json::from_str(source)?;
        let vars = &body["vars"];
        let mut fg = HashMap::new();
        let mut bg = HashMap::new();
        if let Some(colors) = body["colors"].as_object() {
            for (key, value) in colors {
                let resolved = if let Some(name) = value.as_str() { vars.get(name).unwrap_or(value) } else { value };
                fg.insert(key.clone(), color_ansi(resolved, false));
                bg.insert(key.clone(), color_ansi(resolved, true));
            }
        }
        let maps: serde_json::Value = serde_json::from_str(include_str!("themes/symbols.json"))?;
        let mut symbols = HashMap::new();
        if let Some(entries) = maps[preset].as_object() {
            for (key, value) in entries {
                if let Some(value) = value.as_str() {
                    symbols.insert(key.clone(), value.to_owned());
                }
            }
        }
        if let Some(entries) = body.get("symbols").and_then(serde_json::Value::as_object) {
            for (key, value) in entries {
                if let Some(value) = value.as_str() {
                    symbols.insert(key.clone(), value.to_owned());
                }
            }
        }
        let spinner_frames = match preset {
            "ascii" => vec!["|", "/", "-", "\\"],
            "nerd" => vec!["󱑖", "󱑋", "󱑌", "󱑍", "󱑎", "󱑏", "󱑐", "󱑑", "󱑒", "󱑓", "󱑔", "󱑕"],
            _ => vec!["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"],
        }
        .into_iter()
        .map(str::to_owned)
        .collect();
        Ok(Self {
            #[cfg(test)]
            stats_colors: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            identity: NEXT_THEME.fetch_add(1, Ordering::Relaxed),
            revision: 0,
            fg,
            bg,
            symbols,
            spinner_frames,
        })
    }
    pub fn ansi(&self, color: &str) -> &str {
        self.fg.get(color).map(String::as_str).unwrap_or("\x1b[39m")
    }
    /// Existing Theme maps for the fixed graph methods; preserve absence.
    pub(crate) fn fg_value(&self, color: &str) -> Option<&str> {
        self.fg.get(color).map(String::as_str)
    }
    pub(crate) fn bg_value(&self, color: &str) -> Option<&str> {
        self.bg.get(color).map(String::as_str)
    }
    pub(crate) fn symbol_value(&self, key: &str) -> Option<&str> {
        self.symbols.get(key).map(String::as_str)
    }
    pub fn bg_ansi(&self, color: &str) -> &str {
        self.bg.get(color).map(String::as_str).unwrap_or("\x1b[49m")
    }
    pub fn fg(&self, color: &str, text: &str) -> String {
        #[cfg(test)]
        if color == "toolDiffAdded" && text == "+1" {
            self.stats_colors.fetch_add(1, Ordering::Relaxed);
        }
        format!("{}{text}\x1b[39m", self.ansi(color))
    }
    pub fn inverse(&self, text: &str) -> String {
        format!("\x1b[7m{}\x1b[27m", text.replace("\x1b[27m", "\x1b[27m\x1b[7m"))
    }
    pub fn symbol(&self, key: &str) -> &str {
        self.symbols.get(key).map(String::as_str).unwrap_or("")
    }
    pub fn styled_symbol(&self, key: &str, color: &str) -> String {
        self.fg(color, self.symbol(key))
    }
    pub fn status(&self, status: &str, spinner: Option<usize>) -> String {
        if status == "running" {
            if let Some(frame) = spinner {
                return self.fg("accent", &self.spinner_frames[frame % self.spinner_frames.len()]);
            }
        }
        let color = match status {
            "error" => "error",
            "warning" => "warning",
            "success" | "done" => "success",
            _ => "accent",
        };
        self.styled_symbol(&format!("status.{status}"), color)
    }
    pub fn language_icon(&self, language: Option<&str>) -> &str {
        let language = match language.unwrap_or("default") {
            "tsx" => "typescript",
            "jsx" => "javascript",
            "sh" | "shell" => "bash",
            "cpp" => "cpp",
            other => other,
        };
        self.symbols
            .get(&format!("lang.{language}"))
            .map(String::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| self.symbol("lang.default"))
    }
    fn highlight_colors(&self) -> HighlightColors {
        let c = |key| Color::new(self.ansi(key)).expect("bounded ANSI palette");
        HighlightColors {
            comment: c("syntaxComment"),
            keyword: c("syntaxKeyword"),
            function: c("syntaxFunction"),
            variable: c("syntaxVariable"),
            string: c("syntaxString"),
            number: c("syntaxNumber"),
            r#type: c("syntaxType"),
            operator: c("syntaxOperator"),
            punctuation: c("syntaxPunctuation"),
            inserted: Some(c("toolDiffAdded")),
            deleted: Some(c("toolDiffRemoved")),
        }
    }
    pub fn highlight(&self, code: &str, lang: Option<&str>) -> Vec<String> {
        highlight::highlight_code(code, lang, self.highlight_colors()).split('\n').map(str::to_owned).collect()
    }
    /// Explicit reference Host adapter; standalone/custom Markdown defaults remain caller owned.
    pub(crate) fn native_markdown_options(self: &std::sync::Arc<Self>) -> super::markdown::MarkdownOptions {
        let theme = self.clone();
        let colors = self.highlight_colors();
        super::markdown::MarkdownOptions {
            highlighter: Some(std::sync::Arc::new(move |code, language| theme.highlight(code, language))),
            highlight_stream_factory: Some(std::sync::Arc::new(move |language| {
                let stream = highlight::HighlightStream::new(language, colors.clone());
                if !stream.supported() { return Ok(None); }
                Ok(Some(Box::new(stream) as Box<dyn super::markdown::MarkdownHighlightSession>))
            })),
            ..Default::default()
        }
    }
}
fn color_ansi(value: &serde_json::Value, background: bool) -> String {
    let selector = if background { 48 } else { 38 };
    if let Some(number) = value.as_u64() {
        return format!("\x1b[{selector};5;{number}m");
    }
    let hex = value.as_str().unwrap_or("");
    if hex.len() == 7 && hex.starts_with('#') {
        if let Ok(rgb) = u32::from_str_radix(&hex[1..], 16) {
            return format!("\x1b[{selector};2;{};{};{}m", rgb >> 16, (rgb >> 8) & 255, rgb & 255);
        }
    }
    format!("\x1b[{}m", if background { 49 } else { 39 })
}
pub fn language_from_path(path: &str) -> Option<&'static str> {
    let lower = path.to_lowercase();
    let base = lower.rsplit(['/', '\\']).next().unwrap_or(&lower);
    if base.starts_with(".env.") {
        return Some("env");
    }
    if base == "dockerfile" || base.starts_with("dockerfile.") || base == "containerfile" {
        return Some("dockerfile");
    }
    if base == ".emacs" {
        return Some("emacs-lisp");
    }
    if base == "justfile" {
        return Some("just");
    }
    if base == "cmakelists.txt" {
        return Some("cmake");
    }
    let key = lower.rsplit('.').next().unwrap_or(&lower);
    language_extension(key)
}
include!("language_extensions.rs");
