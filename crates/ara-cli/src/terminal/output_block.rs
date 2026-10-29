//! Fixed OMP bordered output block; EDIT requests zero inherited padding.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::{
    render_utils::{truncate, width, wrap},
    theme::Theme,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Pending,
    Running,
    Success,
    Warning,
    Error,
}
pub fn framed(header: &str, lines: &[String], columns: usize, state: State, theme: &Theme) -> Vec<String> {
    let border = if state == State::Error { "error" } else { "borderMuted" };
    let bg = theme.bg_ansi(match state {
        State::Error => "toolErrorBg",
        State::Success => "toolSuccessBg",
        _ => "toolPendingBg",
    });
    let h = theme.symbol("boxRound.horizontal");
    let v = theme.symbol("boxRound.vertical");
    let cap = h.repeat(3);
    let inner = columns.saturating_sub(width(v) * 2);
    let bar = |left: &str, right: &str, label: Option<&str>| {
        let left = format!("{left}{cap}");
        let mut out = theme.fg(border, &left);
        let raw = label.map(|label| format!(" {label} ")).unwrap_or_default();
        let label = truncate(&raw, columns.saturating_sub(width(&left) + width(right)));
        let fill = columns.saturating_sub(width(&left) + width(&label) + width(right));
        out.push_str(&label);
        out.push_str(&theme.fg(border, &h.repeat(fill)));
        out.push_str(&theme.fg(border, right));
        out
    };
    let paint = |line: String| {
        let padding = " ".repeat(columns.saturating_sub(width(&line)));
        let line = format!("{line}{padding}");
        let stable = line
            .replace("\x1b[0m", &format!("\x1b[0m{bg}"))
            .replace("\x1b[m", &format!("\x1b[m{bg}"))
            .replace("\x1b[49m", &format!("\x1b[49m{bg}"));
        format!("{bg}{stable}\x1b[49m")
    };
    let mut out = vec![paint(bar(theme.symbol("boxRound.topLeft"), theme.symbol("boxRound.topRight"), Some(header)))];
    for line in lines {
        for line in line.split('\n') {
            for wrapped in wrap(line.trim_end(), inner) {
                let pad = " ".repeat(inner.saturating_sub(width(&wrapped)));
                out.push(paint(format!("{}{wrapped}{pad}{}", theme.fg(border, v), theme.fg(border, v))));
            }
        }
    }
    out.push(paint(bar(theme.symbol("boxRound.bottomLeft"), theme.symbol("boxRound.bottomRight"), None)));
    out
}
pub fn status_line(title: &str, description: &str, icon: Option<&str>, theme: &Theme) -> String {
    let flatten = |s: &str| s.replace("\r\n", " ").replace(['\r', '\n'], " ");
    let mut line = theme.fg("accent", &flatten(title));
    if let Some(icon) = icon.filter(|i| !i.is_empty()) {
        line = format!("{icon} {line}");
    }
    if !description.is_empty() {
        line.push_str(&format!(": {}", theme.fg("muted", &flatten(description))));
    }
    line
}
