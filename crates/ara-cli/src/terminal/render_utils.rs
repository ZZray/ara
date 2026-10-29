//! Fixed OMP renderer budgets, diagnostic tree and display sanitation.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::{text, theme::Theme, types::Diagnostics};
use ara_rpc::WireString;
use std::sync::OnceLock;
pub const COLLAPSED_HUNKS: usize = 8;
pub const COLLAPSED_LINES: usize = 40;
pub fn sanitize(value: &WireString) -> String {
    text::sanitize_display(value.units())
}
pub fn width(value: &str) -> usize {
    text::visible_width(&value.encode_utf16().collect::<Vec<_>>(), 3) as usize
}
pub fn wrap(value: &str, columns: usize) -> Vec<String> {
    text::wrap_text_with_ansi(&value.encode_utf16().collect::<Vec<_>>(), columns.min(u32::MAX as usize) as u32, 3)
        .into_iter()
        .map(|line| String::from_utf16_lossy(&line))
        .collect()
}
pub fn truncate(value: &str, columns: usize) -> String {
    String::from_utf16_lossy(&text::truncate_to_width(
        &value.encode_utf16().collect::<Vec<_>>(),
        columns.min(u32::MAX as usize) as u32,
        None,
        None,
        3,
    ))
}
pub fn slice(value: &str, start: usize, length: usize) -> String {
    String::from_utf16_lossy(
        &text::slice_with_width(&value.encode_utf16().collect::<Vec<_>>(), start as u32, length as u32, Some(true), 3)
            .text,
    )
}
/// SQLite READ reuses the same existing Host Unicode/ANSI text owner.
pub struct TerminalReadTextMetrics;
impl ara_tools::read_sqlite::ReadTextMetrics for TerminalReadTextMetrics {
    fn width(&self, value: &str) -> Result<usize, ara_agent::ToolError> {
        Ok(width(value))
    }
    fn truncate(&self, value: &str, columns: usize) -> Result<String, ara_agent::ToolError> {
        Ok(truncate(value, columns))
    }
}

pub fn replace_tabs(value: &str) -> String {
    value.replace('\t', "   ")
}
pub fn shorten_path(path: &str) -> String {
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default();
    if home.is_empty() {
        return path.to_owned();
    }
    let windows = home.as_bytes().get(1) == Some(&b':') || home.starts_with("\\\\");
    let has = if windows { path.to_lowercase().starts_with(&home.to_lowercase()) } else { path.starts_with(&home) };
    if has {
        if let Some(suffix) = path.get(home.len()..) {
            if suffix.is_empty() || suffix.starts_with(['/', '\\']) {
                return format!("~{}", suffix.replace('\\', "/"));
            }
        }
    }
    path.to_owned()
}
pub fn middle_truncate(value: &str, max: usize) -> String {
    let cells = width(value);
    if cells <= max {
        return value.to_owned();
    }
    if max <= 1 {
        return "…".to_owned();
    }
    let head = (max - 1) / 2;
    let tail = max - 1 - head;
    format!("{}…{}", slice(value, 0, head), slice(value, cells.saturating_sub(tail), tail))
}
pub fn expand_hint(theme: &Theme) -> String {
    theme.fg(
        "dim",
        &format!("{}ctrl+o: Expand{}", theme.symbol("format.bracketLeft"), theme.symbol("format.bracketRight")),
    )
}
pub fn code_frame_line(marker: &str, number: &str, content: &str, digits: usize) -> String {
    let marker = marker.trim();
    let number = number.trim();
    let gutter = if !marker.is_empty() && !number.is_empty() {
        format!("{marker}{number}")
    } else if !number.is_empty() {
        number.to_owned()
    } else {
        marker.to_owned()
    };
    format!("{:>width$}│{content}", gutter, width = digits + 1)
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiffStats {
    pub added: usize,
    pub removed: usize,
    pub hunks: usize,
    pub lines: usize,
}
pub fn diff_stats(diff: &str) -> DiffStats {
    if diff.is_empty() {
        return DiffStats::default();
    }
    let mut stats = DiffStats::default();
    let mut inside = false;
    for line in diff.split('\n') {
        stats.lines += 1;
        let add = line.starts_with('+');
        let remove = line.starts_with('-');
        stats.added += usize::from(add);
        stats.removed += usize::from(remove);
        let change = add || remove;
        if change && !inside {
            stats.hunks += 1;
            inside = true;
        } else if !change {
            inside = false;
        }
    }
    stats
}
#[derive(Clone, Debug)]
pub struct TruncatedDiff {
    pub text: String,
    pub hidden_hunks: usize,
    pub hidden_lines: usize,
}
#[derive(Clone)]
struct Segment<'a> {
    lines: Vec<&'a str>,
    change: bool,
    ellipsis: bool,
}
pub fn truncate_diff_by_hunk(diff: &str, max_hunks: usize, max_lines: usize) -> TruncatedDiff {
    let lines = if diff.is_empty() { vec![] } else { diff.split('\n').collect::<Vec<_>>() };
    let total = diff_stats(diff);
    if lines.len() <= max_lines && total.hunks <= max_hunks {
        return TruncatedDiff { text: diff.to_owned(), hidden_hunks: 0, hidden_lines: 0 };
    }
    let mut segments: Vec<Segment<'_>> = Vec::new();
    for line in &lines {
        let change = line.starts_with('+') || line.starts_with('-');
        let ellipsis = line.trim_start().starts_with("...") || line.trim().is_empty();
        if ellipsis || segments.last().is_none_or(|last| last.change != change || last.ellipsis) {
            segments.push(Segment { lines: vec![line], change, ellipsis });
        } else {
            segments.last_mut().unwrap().lines.push(line);
        }
    }
    let changes: usize = segments.iter().filter(|seg| seg.change).map(|seg| seg.lines.len()).sum();
    let mut kept = Vec::new();
    let mut hunks = 0;
    let mut source_lines = 0;
    if changes > max_lines {
        for seg in &segments {
            if kept.len() >= max_lines {
                break;
            }
            if seg.change {
                if hunks >= max_hunks {
                    break;
                }
                hunks += 1;
            }
            let take = seg.lines.len().min(max_lines - kept.len());
            kept.extend_from_slice(&seg.lines[..take]);
        }
        source_lines = kept.len();
    } else {
        let context_budget = max_lines - changes;
        let context_total: usize = segments.iter().filter(|seg| !seg.change).map(|seg| seg.lines.len()).sum();
        if context_total <= context_budget {
            for seg in &segments {
                if seg.change {
                    if hunks >= max_hunks {
                        break;
                    }
                    hunks += 1;
                }
                kept.extend_from_slice(&seg.lines);
                source_lines += seg.lines.len();
            }
        } else {
            let ratio = context_budget as f64 / context_total as f64;
            let mut remaining = context_budget;
            for (index, seg) in segments.iter().enumerate() {
                if seg.change {
                    if hunks >= max_hunks {
                        break;
                    }
                    hunks += 1;
                    kept.extend_from_slice(&seg.lines);
                    source_lines += seg.lines.len();
                    continue;
                }
                if remaining == 0 {
                    continue;
                }
                let allowed = remaining.min(((seg.lines.len() as f64 * ratio).floor() as usize).max(1));
                let before = kept.len();
                let added;
                if seg.ellipsis || seg.lines.len() <= allowed {
                    kept.extend_from_slice(&seg.lines[..allowed]);
                    added = allowed;
                } else {
                    let before_change = segments.get(index + 1).is_some_and(|s| s.change);
                    let after_change = index > 0 && segments[index - 1].change;
                    if before_change && after_change {
                        let budget = if allowed >= 3 { allowed - 1 } else { allowed };
                        let first = budget.div_ceil(2);
                        let last = budget - first;
                        kept.extend_from_slice(&seg.lines[..first]);
                        if allowed >= 3 {
                            kept.push("");
                        }
                        if last > 0 {
                            kept.extend_from_slice(&seg.lines[seg.lines.len() - last..]);
                        }
                        added = budget;
                    } else if before_change {
                        kept.extend_from_slice(&seg.lines[seg.lines.len() - allowed..]);
                        added = allowed;
                    } else if after_change {
                        kept.extend_from_slice(&seg.lines[..allowed]);
                        added = allowed;
                    } else {
                        let take = allowed.min(2);
                        kept.extend_from_slice(&seg.lines[..take]);
                        added = take;
                    }
                }
                source_lines += added;
                remaining = remaining.saturating_sub(kept.len() - before);
            }
        }
    }
    TruncatedDiff {
        text: kept.join("\n"),
        hidden_hunks: total.hunks.saturating_sub(hunks),
        hidden_lines: lines.len().saturating_sub(source_lines),
    }
}
#[derive(Clone)]
struct Diagnostic {
    path: String,
    line: u32,
    col: u32,
    severity: String,
    message: String,
    code: Option<String>,
}
fn parsed_diag(message: &str) -> Option<Diagnostic> {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r"^(.+?):(\d+):(\d+)\s+\[(\w+)\]\s+(?:\[([^\]]+)\]\s+)?(.+?)(?:\s+\(([^)]+)\))?$").unwrap()
    });
    let m = re.captures(message)?;
    Some(Diagnostic {
        path: replace_tabs(&m[1]),
        line: m[2].parse().ok()?,
        col: m[3].parse().ok()?,
        severity: m[4].to_owned(),
        message: replace_tabs(&m[6]),
        code: m.get(7).map(|m| replace_tabs(m.as_str())),
    })
}
fn severity_rank(value: &str) -> usize {
    match value {
        "error" => 0,
        "warning" => 1,
        "info" => 2,
        _ => 3,
    }
}
pub fn diagnostics(diag: &Diagnostics, expanded: bool, theme: &Theme) -> String {
    if diag.messages.is_empty() {
        return String::new();
    }
    let mut files: Vec<(String, Vec<Diagnostic>)> = Vec::new();
    let mut unparsed = Vec::new();
    for message in &diag.messages {
        let message = sanitize(message);
        if let Some(d) = parsed_diag(&message) {
            if let Some((_, list)) = files.iter_mut().find(|(path, _)| path == &d.path) {
                list.push(d);
            } else {
                files.push((d.path.clone(), vec![d]));
            }
        } else {
            unparsed.push(replace_tabs(&message));
        }
    }
    for (_, list) in &mut files {
        list.sort_by(|a, b| {
            severity_rank(&a.severity)
                .cmp(&severity_rank(&b.severity))
                .then(a.line.cmp(&b.line))
                .then(a.col.cmp(&b.col))
                .then(a.message.cmp(&b.message))
        });
    }
    let icon = theme.status(if diag.errored { "error" } else { "warning" }, None);
    let summary = replace_tabs(&sanitize(&diag.summary.0));
    let tag = if summary.is_empty() { String::new() } else { format!(" {}", theme.fg("dim", &format!("({summary})"))) };
    let mut output = format!("\n\n{icon} {}{tag}", theme.fg("toolTitle", "Diagnostics"));
    let max = if expanded { diag.messages.len() } else { 5 };
    let total = diag.messages.len();
    let mut shown = 0;
    for (fi, (path, list)) in files.iter().enumerate() {
        if shown >= max {
            break;
        }
        let after: usize = files[fi + 1..].iter().map(|(_, d)| d.len()).sum::<usize>() + unparsed.len();
        let last_file = fi + 1 == files.len() && unparsed.is_empty() && shown + list.len() + after <= max;
        let branch = theme.symbol(if last_file { "tree.last" } else { "tree.branch" });
        let icon = theme.fg("muted", theme.language_icon(super::theme::language_from_path(path)));
        output.push_str(&format!("\n {} {icon} {}", theme.fg("dim", branch), theme.fg("accent", path)));
        for (di, d) in list.iter().enumerate() {
            if shown >= max {
                break;
            }
            let last = di + 1 == list.len() || shown + 1 >= max;
            let branch = if last_file {
                format!("  {}", theme.symbol(if last { "tree.last" } else { "tree.branch" }))
            } else {
                format!(
                    "{} {}",
                    theme.symbol("tree.vertical"),
                    theme.symbol(if last { "tree.last" } else { "tree.branch" })
                )
            };
            let sev = if d.severity == "error" {
                "error"
            } else if d.severity == "warning" {
                "warning"
            } else {
                "info"
            };
            let color = if sev == "info" { "toolOutput" } else { sev };
            let code = d.code.as_ref().map(|code| theme.fg("dim", &format!(" ({code})"))).unwrap_or_default();
            output.push_str(&format!(
                "\n {} {}{} {}{code}",
                theme.fg("dim", &branch),
                theme.status(sev, None),
                theme.fg("dim", &format!(":{}:{}", d.line, d.col)),
                theme.fg(color, &d.message)
            ));
            shown += 1;
        }
    }
    for (index, message) in unparsed.iter().enumerate() {
        if shown >= max {
            break;
        }
        let last = shown + 1 >= total && index + 1 == unparsed.len();
        let color = if message.contains("[error]") {
            "error"
        } else if message.contains("[warning]") {
            "warning"
        } else {
            "dim"
        };
        output.push_str(&format!(
            "\n {} {}",
            theme.fg("dim", theme.symbol(if last { "tree.last" } else { "tree.branch" })),
            theme.fg(color, message)
        ));
        shown += 1;
    }
    if total > shown {
        output.push_str(&format!(
            "\n {} {} {}",
            theme.fg("dim", theme.symbol("tree.last")),
            theme.fg("muted", &format!("… {} more", total - shown)),
            expand_hint(theme)
        ));
    }
    output
}

/// Fixed OMP host-owned presentation leaves; no persistence or Core state.
#[derive(Clone, Debug, PartialEq)]
pub struct TerminalPresentationSettings {
    pub cache_miss_marker: bool,
    pub collapse_compacted: bool,
    pub smooth_streaming: bool,
    pub image_columns: f64,
    pub image_rows: f64,
}
impl Default for TerminalPresentationSettings {
    fn default() -> Self {
        Self {
            cache_miss_marker: false,
            collapse_compacted: true,
            smooth_streaming: true,
            image_columns: 100.0,
            image_rows: 20.0,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageOptions {
    pub max_width_cells: f64,
    pub max_height_cells: Option<f64>,
}
pub fn resolve_image_options(
    settings: Option<&TerminalPresentationSettings>,
    viewport_rows: Option<usize>,
) -> ImageOptions {
    let defaults = TerminalPresentationSettings::default();
    let settings = settings.unwrap_or(&defaults);
    let rows = settings.image_rows.max(0.0);
    let fraction = viewport_rows.filter(|&n| n > 0).map(|n| (n as f64 * 0.6).floor()).unwrap_or(0.0);
    let max_height_cells = if rows == 0.0 {
        (fraction > 0.0).then_some(fraction)
    } else if fraction > 0.0 {
        Some(rows.min(fraction))
    } else {
        Some(rows)
    };
    ImageOptions { max_width_cells: settings.image_columns, max_height_cells }
}
