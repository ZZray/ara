//! Fixed OMP diff.ts body, retaining the native UTF16 word-diff algorithm.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::{
    render_utils::{code_frame_line, replace_tabs, sanitize},
    theme::{Theme, language_from_path},
};
use ara_rpc::WireString;
use std::{collections::HashMap, sync::OnceLock};
#[derive(Clone)]
pub(crate) struct Parsed {
    pub(crate) prefix: char,
    pub(crate) number: String,
    pub(crate) content: String,
}
pub(crate) fn parse(line: &str) -> Option<Parsed> {
    static CANON: OnceLock<regex::Regex> = OnceLock::new();
    static LEGACY: OnceLock<regex::Regex> = OnceLock::new();
    let canon = CANON.get_or_init(|| regex::Regex::new(r"^([+\-\s])(\s*[0-9]+)\|(.*)$").unwrap());
    let legacy = LEGACY.get_or_init(|| regex::Regex::new(r"^([+\-\s])(?:(\s*[0-9]+)\s)?(.*)$").unwrap());
    if let Some(m) = canon.captures(line) {
        return Some(Parsed { prefix: m[1].chars().next()?, number: m[2].to_owned(), content: m[3].to_owned() });
    }
    let m = legacy.captures(line)?;
    Some(Parsed {
        prefix: m[1].chars().next()?,
        number: m.get(2).map(|m| m.as_str()).unwrap_or("").to_owned(),
        content: m[3].to_owned(),
    })
}
fn indent(text: &str) -> String {
    let n = text.chars().take_while(|c| matches!(c, ' ' | '\t')).count();
    if n == 0 {
        return replace_tabs(text);
    }
    let mut out = String::new();
    for c in text.chars().take(n) {
        out.push_str(if c == '\t' { "\x1b[2m → \x1b[22m" } else { "\x1b[2m·\x1b[22m" });
    }
    out.push_str(&replace_tabs(&text.chars().skip(n).collect::<String>()));
    out
}
fn intra(old: &str, new: &str, theme: &Theme) -> (String, String) {
    let mut removed = String::new();
    let mut added = String::new();
    let (mut first_removed, mut first_added) = (true, true);
    for part in
        ara_diff::diff_words_u16(&old.encode_utf16().collect::<Vec<_>>(), &new.encode_utf16().collect::<Vec<_>>())
    {
        let mut value = String::from_utf16_lossy(&part.value);
        if part.removed || part.added {
            let (first, out) =
                if part.removed { (&mut first_removed, &mut removed) } else { (&mut first_added, &mut added) };
            if *first {
                let whitespace = value.len() - value.trim_start_matches(char::is_whitespace).len();
                out.push_str(&value[..whitespace]);
                value = value[whitespace..].to_owned();
                *first = false;
            }
            if !value.is_empty() {
                out.push_str(&theme.inverse(&value));
            }
        } else {
            removed.push_str(&value);
            added.push_str(&value);
        }
    }
    (removed, added)
}
pub fn render_diff(diff: &WireString, file_path: Option<&str>, theme: &Theme) -> String {
    let text = sanitize(diff);
    render_scalar_diff(&text, file_path, theme)
}
pub fn render_scalar_diff(diff: &str, file_path: Option<&str>, theme: &Theme) -> String {
    // This is the original sanitizeText display boundary, never a canonical
    // Graph projection or authority input. Native text primitives stay UTF16.
    let text = sanitize(&WireString::from(diff));
    let lines = text.split('\n').collect::<Vec<_>>();
    let parsed = lines.iter().map(|line| parse(line)).collect::<Vec<_>>();
    let digits = parsed.iter().filter_map(|p| p.as_ref()).map(|p| p.number.trim().len()).max().unwrap_or(0).max(3);
    let mut highlights = HashMap::new();
    if let Some(lang) = file_path.and_then(language_from_path) {
        let mut indices = Vec::new();
        let mut contents = Vec::new();
        let flush = |indices: &mut Vec<usize>, contents: &mut Vec<String>, highlights: &mut HashMap<usize, String>| {
            if contents.is_empty() {
                return;
            }
            let rendered = theme.highlight(&contents.join("\n"), Some(lang));
            for (k, index) in indices.iter().enumerate() {
                highlights.insert(*index, rendered.get(k).cloned().unwrap_or_else(|| contents[k].clone()));
            }
            indices.clear();
            contents.clear();
        };
        for (j, p) in parsed.iter().enumerate() {
            if let Some(p) = p.as_ref().filter(|p| p.prefix == ' ' && p.content != "..." && p.content != "…") {
                indices.push(j);
                contents.push(p.content.clone());
            } else {
                flush(&mut indices, &mut contents, &mut highlights);
            }
        }
        flush(&mut indices, &mut contents, &mut highlights);
    }
    let mut previous = String::new();
    let mut result = Vec::new();
    let mut i = 0;
    fn format(prefix: char, number: &str, content: &str, digits: usize, previous: &mut String) -> String {
        let trimmed = number.trim();
        if trimmed.is_empty() {
            previous.clear();
            return format!("{prefix}{content}");
        }
        let display = if trimmed == previous.as_str() { "" } else { trimmed };
        let output = code_frame_line(&prefix.to_string(), display, content, digits);
        *previous = trimmed.to_owned();
        output
    }
    while i < lines.len() {
        let Some(p) = &parsed[i] else {
            previous.clear();
            let trimmed = lines[i].trim();
            let gap = trimmed.is_empty() || trimmed == "..." || trimmed == "…";
            result.push(theme.fg("toolDiffContext", if gap { "…" } else { lines[i] }));
            i += 1;
            continue;
        };
        if p.prefix == '-' {
            let mut removals = Vec::new();
            while i < parsed.len() && parsed[i].as_ref().is_some_and(|p| p.prefix == '-') {
                removals.push(parsed[i].as_ref().unwrap().clone());
                i += 1;
            }
            let mut additions = Vec::new();
            while i < parsed.len() && parsed[i].as_ref().is_some_and(|p| p.prefix == '+') {
                additions.push(parsed[i].as_ref().unwrap().clone());
                i += 1;
            }
            if removals.len() == 1 && additions.len() == 1 {
                let (r, a) = intra(&replace_tabs(&removals[0].content), &replace_tabs(&additions[0].content), theme);
                result.push(
                    theme.fg("toolDiffRemoved", &format('-', &removals[0].number, &indent(&r), digits, &mut previous)),
                );
                result.push(
                    theme.fg("toolDiffAdded", &format('+', &additions[0].number, &indent(&a), digits, &mut previous)),
                );
            } else {
                for r in removals {
                    result.push(
                        theme
                            .fg("toolDiffRemoved", &format('-', &r.number, &indent(&r.content), digits, &mut previous)),
                    );
                }
                for a in additions {
                    result.push(
                        theme.fg("toolDiffAdded", &format('+', &a.number, &indent(&a.content), digits, &mut previous)),
                    );
                }
            }
        } else if p.prefix == '+' {
            result.push(theme.fg("toolDiffAdded", &format('+', &p.number, &indent(&p.content), digits, &mut previous)));
            i += 1;
        } else {
            let content = highlights.get(&i).map(|h| replace_tabs(h)).unwrap_or_else(|| indent(&p.content));
            result.push(theme.fg("toolDiffContext", &format(' ', &p.number, &content, digits, &mut previous)));
            i += 1;
        }
    }
    result.join("\n")
}
