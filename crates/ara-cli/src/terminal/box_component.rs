//! Fixed OMP 596f2da Box static layout, padding/background/border and child cache.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
#[path = "static_line_widths.rs"]
pub mod line_widths;
use super::{render_utils, text, theme::Theme};
use std::sync::Arc;
pub type Colorizer = Arc<dyn Fn(&str) -> String + Send + Sync>;
pub trait StaticComponent: Send {
    fn render(&mut self, width: usize) -> Arc<Vec<String>>;
    fn invalidate(&mut self, theme: Arc<Theme>);
    fn set_ignore_tight(&mut self, _ignore: bool) {}
}
#[derive(Clone)]
pub struct BoxBorder {
    pub top_left: String,
    pub top_right: String,
    pub bottom_left: String,
    pub bottom_right: String,
    pub horizontal: String,
    pub vertical: String,
    pub color: Option<String>,
}
struct Cache {
    width: usize,
    width_epoch: u64,
    child_widths: Vec<Option<Arc<Vec<usize>>>>,
    theme: (u64, u64),
    bg_sample: String,
    border_sample: String,
    children: Vec<Arc<Vec<String>>>,
    rows: Arc<Vec<String>>,
}
pub struct BoxComponent {
    children: Vec<Box<dyn StaticComponent>>,
    padding_x: usize,
    padding_y: usize,
    bg: Option<String>,
    bg_fn: Option<Colorizer>,
    border_colorizer: Option<Colorizer>,
    border: Option<BoxBorder>,
    theme: Arc<Theme>,
    ignore_tight: bool,
    tight: bool,
    cache: Option<Cache>,
}
impl BoxComponent {
    pub fn new(
        padding_x: usize,
        padding_y: usize,
        bg: Option<String>,
        border: Option<BoxBorder>,
        theme: Arc<Theme>,
    ) -> Self {
        Self {
            children: Vec::new(),
            padding_x,
            padding_y,
            bg,
            bg_fn: None,
            border_colorizer: None,
            border,
            theme,
            ignore_tight: false,
            tight: false,
            cache: None,
        }
    }
    pub fn set_ignore_tight(&mut self, ignore: bool) {
        self.ignore_tight = ignore;
        self.cache = None;
    }
    pub fn set_tight(&mut self, tight: bool) {
        if self.tight != tight {
            self.tight = tight;
            self.cache = None;
        }
    }
    pub fn add_child(&mut self, mut child: Box<dyn StaticComponent>) {
        if self.ignore_tight {
            child.set_ignore_tight(true);
        }
        self.children.push(child);
        self.cache = None;
    }
    pub fn remove_child(&mut self, index: usize) {
        if index < self.children.len() {
            self.children.remove(index);
            self.cache = None;
        }
    }
    pub fn clear(&mut self) {
        self.children.clear();
        self.cache = None;
    }
    pub fn set_padding_x(&mut self, padding: usize) {
        if self.padding_x != padding {
            self.padding_x = padding;
            self.cache = None;
        }
    }
    pub fn set_padding_y(&mut self, padding: usize) {
        if self.padding_y != padding {
            self.padding_y = padding;
            self.cache = None;
        }
    }
    pub fn set_bg(&mut self, bg: Option<String>) {
        self.bg = bg;
    }
    pub fn set_bg_fn(&mut self, bg: Option<Colorizer>) {
        self.bg_fn = bg;
    }
    pub fn set_border_colorizer(&mut self, color: Option<Colorizer>) {
        self.border_colorizer = color;
        self.cache = None;
    }
    pub fn set_border(&mut self, border: Option<BoxBorder>) {
        self.border = border;
        self.cache = None;
    }
    pub fn invalidate(&mut self, theme: Arc<Theme>) {
        self.theme = theme.clone();
        self.cache = None;
        for child in &mut self.children {
            child.invalidate(theme.clone());
        }
    }
    pub fn render(&mut self, width: usize) -> Arc<Vec<String>> {
        let padding_x =
            if self.tight && !self.ignore_tight { self.padding_x.saturating_sub(1) } else { self.padding_x };
        let border = self.border.as_ref().filter(|_| width.saturating_sub(2) >= padding_x * 2 + 1 && width >= 2);
        let inner = width.saturating_sub(if border.is_some() { 2 } else { 0 });
        let content = inner.saturating_sub(padding_x * 2).max(1);
        let bg_sample = self
            .bg_fn
            .as_ref()
            .map(|f| f("test"))
            .unwrap_or_else(|| self.bg.as_ref().map(|bg| self.theme.bg_ansi(bg)).unwrap_or("").to_owned());
        let border_sample = border
            .map(|b| {
                format!(
                    "{}{}{}",
                    self.border_colorizer.as_ref().map(|f| f("|")).unwrap_or_else(|| b
                        .color
                        .as_ref()
                        .map(|c| self.theme.fg(c, "|"))
                        .unwrap_or_else(|| "|".into())),
                    b.top_left,
                    b.vertical
                )
            })
            .unwrap_or_default();
        // Fixed order: sample callbacks before the epoch read and child side effects.
        let width_epoch = text::get_width_config_epoch();
        let children: Vec<_> = self.children.iter_mut().map(|child| child.render(content)).collect();
        let child_widths: Vec<_> = children.iter().map(line_widths::get_published_line_widths).collect();
        let theme = (self.theme.identity, self.theme.revision);
        if let Some(cache) = &self.cache
            && cache.width == width
            && cache.width_epoch == width_epoch
            && cache.width_epoch == text::get_width_config_epoch()
            && cache.child_widths.iter().zip(&child_widths).all(|(a, b)| match (a, b) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            })
            && cache.theme == theme
            && cache.bg_sample == bg_sample
            && cache.border_sample == border_sample
            && cache.children.len() == children.len()
            && children.iter().zip(&cache.children).all(|(a, b)| Arc::ptr_eq(a, b))
        {
            return cache.rows.clone();
        }
        let mut rows = Vec::new();
        let mut result_widths = Vec::new();
        if children.iter().any(|child| !child.is_empty()) {
            let background = |row: String| {
                if let Some(f) = &self.bg_fn {
                    return f(&row);
                }
                if bg_sample.is_empty() {
                    row
                } else {
                    format!(
                        "{}{}\x1b[49m",
                        bg_sample,
                        row.replace("\x1b[0m", &format!("\x1b[0m{bg_sample}"))
                            .replace("\x1b[49m", &format!("\x1b[49m{bg_sample}"))
                    )
                }
            };
            let padded = |row: String, cells: usize| {
                (background(format!("{row}{}", " ".repeat(inner.saturating_sub(cells)))), cells.max(inner))
            };
            let mut interior = Vec::new();
            for _ in 0..self.padding_y {
                let (row, width) = padded(String::new(), 0);
                interior.push(row);
                result_widths.push(width);
            }
            for (i, child) in children.iter().enumerate() {
                for (j, line) in child.iter().enumerate() {
                    let raw = format!("{}{line}", " ".repeat(padding_x));
                    let cells = if padding_x == 0 {
                        child_widths[i]
                            .as_ref()
                            .and_then(|w| w.get(j))
                            .copied()
                            .unwrap_or_else(|| render_utils::width(&raw))
                    } else {
                        render_utils::width(&raw)
                    };
                    let (row, width) = padded(raw, cells);
                    interior.push(row);
                    result_widths.push(width);
                }
            }
            for _ in 0..self.padding_y {
                let (row, width) = padded(String::new(), 0);
                interior.push(row);
                result_widths.push(width);
            }
            if let Some(border) = border {
                let paint = |text: &str| {
                    self.border_colorizer.as_ref().map(|f| f(text)).unwrap_or_else(|| {
                        border.color.as_ref().map_or_else(|| text.to_owned(), |color| self.theme.fg(color, text))
                    })
                };
                let rule = border.horizontal.repeat(inner);
                let side = paint(&border.vertical);
                rows.push(paint(&format!("{}{rule}{}", border.top_left, border.top_right)));
                rows.extend(interior.into_iter().map(|row| format!("{side}{row}{side}")));
                rows.push(paint(&format!("{}{rule}{}", border.bottom_left, border.bottom_right)));
            } else {
                rows = interior;
            }
        }
        let rows = Arc::new(rows);
        // Measuring rows is not an atomic snapshot of all width reads. If a
        // child/configuration changes it during this frame, return the rows
        // without publishing or saving a cache under the newer epoch.
        if width_epoch == text::get_width_config_epoch() {
            if border.is_none() && self.bg.is_none() && self.bg_fn.is_none() {
                line_widths::publish_at_epoch(&rows, Arc::new(result_widths), width_epoch)
                    .expect("Box derives one published width for every result row");
            }
            self.cache = Some(Cache {
                width,
                width_epoch,
                child_widths,
                theme,
                bg_sample,
                border_sample,
                children,
                rows: rows.clone(),
            });
        } else {
            self.cache = None;
        }
        rows
    }
}

#[cfg(test)]
#[path = "box_original_tests.rs"]
mod original_tests;
