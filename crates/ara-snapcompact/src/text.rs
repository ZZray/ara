//! Fixed OMP transcript serializer, atomic data-URL healing and font-aware folds.
use crate::{DIM_OFF, DIM_ON, NEWLINE_GLYPH, NormalizeOptions, Renderability, SerializeOptions};
use ara_prompt::js::{entries, is_space, json_stringify, trim, utf16_len};
use regex::Regex;
use serde_json::Value;
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    sync::OnceLock,
};
use unicode_general_category::{GeneralCategory as Gc, get_general_category};
use unicode_normalization::UnicodeNormalization;

fn regex_cached(slot: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    slot.get_or_init(|| Regex::new(pattern).expect("fixed snapcompact regex"))
}
pub(crate) fn utf16_slice(text: &str, start: usize, end: usize) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    String::from_utf16_lossy(&units[start.min(units.len())..end.min(units.len()).max(start.min(units.len()))])
}
pub fn strip_dim_markers(text: &str) -> String {
    text.chars().filter(|&c| c != DIM_ON && c != DIM_OFF).collect()
}
pub(crate) fn to_plain_text(text: &str) -> String {
    strip_dim_markers(text).replace(NEWLINE_GLYPH, "\n")
}
fn truncate(text: &str, cap: f64, head_ratio: f64) -> String {
    let len = utf16_len(text);
    if len as f64 <= cap {
        return text.into();
    }
    let head = (cap * head_ratio.clamp(0.0, 1.0)).round() as usize;
    let tail = (cap - head as f64).max(0.0) as usize;
    format!(
        "{} […{}ch elided…] {}",
        utf16_slice(text, 0, head),
        ara_prompt::js::f64_to_string(len as f64 - cap),
        if tail > 0 { utf16_slice(text, len.saturating_sub(tail), len) } else { String::new() }
    )
}
fn markdown_opener(text: &str, at: usize, cursor: usize) -> Option<usize> {
    let mut end = at;
    while end > cursor {
        let (offset, ch) = text[..end].char_indices().next_back()?;
        if !is_space(ch) {
            break;
        }
        end = offset;
    }
    if end < cursor + 2 || text.as_bytes().get(end - 2..end) != Some(b"](") {
        return None;
    }
    let mut opener = None;
    for (offset, ch) in text[cursor..end - 2].char_indices().rev() {
        if ch == ']' || ch == '\n' {
            break;
        }
        if ch == '[' {
            opener = Some(cursor + offset);
        }
    }
    opener.map(|start| if start > cursor && text.as_bytes()[start - 1] == b'!' { start - 1 } else { start })
}
pub(crate) fn elide_data_urls(text: &str, archive: bool) -> String {
    static ATOM: OnceLock<Regex> = OnceLock::new();
    static MARKER: OnceLock<Regex> = OnceLock::new();
    static CANONICAL: OnceLock<Regex> = OnceLock::new();
    let ws = r"[\t\n\x0B\x0C\r \x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}\x{FEFF}]";
    let marker = r"\[(?:…|\.{3})[0-9]+ch elided(?:…|\.{3})\]";
    let token = r"[A-Za-z0-9_!#$%&'*+.^|~-]+";
    let pattern = format!(
        r"(?i)data:([A-Za-z][A-Za-z0-9_.+-]*/[A-Za-z0-9_.+-]+(?:;{token}={token})*);base64,([A-Za-z0-9+/=]*(?:{ws}*{marker}{ws}*[A-Za-z0-9+/=]*)?)({ws}*\))?"
    );
    let atoms = regex_cached(&ATOM, &pattern);
    let marker_re = regex_cached(&MARKER, &format!("{ws}*{marker}{ws}*"));
    let canonical =
        regex_cached(&CANONICAL, r"^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{4}|[A-Za-z0-9+/]{3}=|[A-Za-z0-9+/]{2}==)$");
    let mut out = String::new();
    let mut cursor = 0;
    for captures in atoms.captures_iter(text) {
        let atom = captures.get(0).expect("full regex match");
        let payload = captures.get(2).map(|m| m.as_str()).unwrap_or("");
        let marker = marker_re.find(payload);
        if !archive && marker.is_none() && !canonical.is_match(payload) && utf16_len(payload) < 40 {
            out.push_str(&text[cursor..atom.end()]);
            cursor = atom.end();
            continue;
        }
        let length = if let Some(marker) = marker {
            let digits: String = marker.as_str().chars().filter(char::is_ascii_digit).collect();
            utf16_len(payload) as f64 - utf16_len(marker.as_str()) as f64 + digits.parse::<f64>().unwrap_or(0.0)
        } else {
            utf16_len(payload) as f64
        };
        let placeholder = format!(
            "[data URL omitted: {}, {} base64 chars]",
            captures.get(1).map(|m| m.as_str()).unwrap_or(""),
            ara_prompt::js::f64_to_string(length)
        );
        let opener = markdown_opener(text, atom.start(), cursor);
        let closer = captures.get(3).map(|m| m.as_str());
        out.push_str(&text[cursor..opener.unwrap_or(atom.start())]);
        if opener.is_some() && closer.is_some() {
            out.push_str(&placeholder);
        } else {
            if let Some(start) = opener {
                out.push_str(&text[start..atom.start()]);
            }
            out.push_str(&placeholder);
            if let Some(closer) = closer {
                out.push_str(closer);
            }
        }
        cursor = atom.end();
    }
    out.push_str(&text[cursor..]);
    out
}
fn content_text(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.into();
    }
    value.as_array().into_iter().flatten().filter(|v| v["type"] == "text").filter_map(|v| v["text"].as_str()).collect()
}
fn string(value: &Value) -> &str {
    value.as_str().unwrap_or("")
}
fn push_part(parts: &mut Vec<String>, last: &mut String, prefix: &str, text: &str) {
    if last == prefix
        && let Some(part) = parts.last_mut()
    {
        if !part.ends_with('\n') && !text.starts_with('\n') {
            part.push('\n');
        }
        part.push_str(text);
    } else {
        parts.push(format!("{prefix}{text}"));
        *last = prefix.into();
    }
}
fn flush_assistant(parts: &mut Vec<String>, last: &mut String, thinking: &mut Vec<String>, text: &mut Vec<String>) {
    if !thinking.is_empty() {
        push_part(parts, last, "¶think:", &thinking.join("\n"));
        thinking.clear();
    }
    if !text.is_empty() {
        push_part(parts, last, "¶ai:", &text.join("\n"));
        text.clear();
    }
}
pub fn serialize_conversation(messages: &[Value], options: &SerializeOptions) -> String {
    let mut useless = HashSet::new();
    let mut results = HashMap::new();
    for message in messages {
        if message["role"] != "toolResult" {
            continue;
        }
        let id = string(&message["toolCallId"]);
        if message["useless"] == true && message["isError"] != true {
            useless.insert(id);
            continue;
        }
        let text = content_text(&message["content"]);
        if !text.is_empty() {
            results.insert(id, text);
        }
    }
    let result_block = |text: &str| {
        let body = truncate(
            &elide_data_urls(&strip_dim_markers(text), false),
            options.tool_result_max_chars,
            options.truncate_head_ratio,
        );
        format!("<out>\n{}\n</out>", if options.dim_tool_results { format!("{DIM_ON}{body}{DIM_OFF}") } else { body })
    };
    let mut merged = HashSet::new();
    let mut parts = Vec::new();
    let mut last = String::new();
    for message in messages {
        match string(&message["role"]) {
            "user" => {
                let text = content_text(&message["content"]);
                if !text.is_empty() {
                    push_part(&mut parts, &mut last, "¶user:", &strip_dim_markers(&text));
                }
            }
            "assistant" => {
                let mut thinking = Vec::new();
                let mut text = Vec::new();
                for block in message["content"].as_array().into_iter().flatten() {
                    match string(&block["type"]) {
                        "text" => {
                            let value = strip_dim_markers(string(&block["text"]));
                            if !trim(&value).is_empty() {
                                text.push(value);
                            }
                        }
                        "thinking" if options.include_thinking => {
                            let value = strip_dim_markers(string(&block["thinking"]));
                            if !trim(&value).is_empty() {
                                thinking.push(value);
                            }
                        }
                        "toolCall" => {
                            let id = string(&block["id"]);
                            if useless.contains(id) {
                                continue;
                            }
                            flush_assistant(&mut parts, &mut last, &mut thinking, &mut text);
                            let intent =
                                block["intent"].as_str().or_else(|| block["arguments"]["i"].as_str()).unwrap_or("");
                            let intent = strip_dim_markers(intent)
                                .split(is_space)
                                .filter(|s| !s.is_empty())
                                .collect::<Vec<_>>()
                                .join(" ");
                            let args = block["arguments"]
                                .as_object()
                                .map(|args| {
                                    entries(args)
                                        .into_iter()
                                        .filter(|(k, _)| k.as_str() != "i")
                                        .map(|(key, value)| {
                                            format!(
                                                "{key}={}",
                                                truncate(
                                                    &elide_data_urls(&json_stringify(value), false),
                                                    options.tool_arg_max_chars,
                                                    options.truncate_head_ratio
                                                )
                                            )
                                        })
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                })
                                .unwrap_or_default();
                            let args = truncate(&args, options.tool_call_max_chars, options.truncate_head_ratio);
                            let mut call = format!("{}({args})", string(&block["name"]));
                            if !intent.is_empty() {
                                call.push_str("//");
                                call.push_str(&intent);
                            }
                            if let Some(result) = results.get(id) {
                                merged.insert(id);
                                call.push('\n');
                                call.push_str(&result_block(result));
                            }
                            push_part(&mut parts, &mut last, "¶call:", &call);
                        }
                        _ => {}
                    }
                }
                flush_assistant(&mut parts, &mut last, &mut thinking, &mut text);
            }
            "toolResult" => {
                let id = string(&message["toolCallId"]);
                if !useless.contains(id)
                    && !merged.contains(id)
                    && let Some(result) = results.get(id)
                {
                    push_part(&mut parts, &mut last, "¶call:", &format!("\n{}", result_block(result)));
                }
            }
            _ => {}
        }
    }
    parts.join("\n\n")
}

fn char_fold(c: char) -> Option<&'static str> {
    Some(match c {
        '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}' | '\u{2032}' | '\u{2035}' => "'",
        '\u{201c}' | '\u{201d}' | '\u{201e}' | '\u{2033}' | '\u{2036}' => "\"",
        '\u{2039}' => "<",
        '\u{203a}' => ">",
        '\u{2010}'..='\u{2015}' | '\u{2212}' => "-",
        '\u{2044}' => "/",
        '\u{2024}' => ".",
        '\u{2025}' => "..",
        '\u{2026}' | '\u{22ef}' => "...",
        '\u{2022}' | '\u{2023}' | '\u{2219}' | '\u{25cf}' | '\u{25a0}' | '\u{25aa}' => "*",
        '\u{2043}' => "-",
        '\u{2190}' => "<-",
        '\u{2191}' => "^",
        '\u{2192}' => "->",
        '\u{2193}' => "v",
        '\u{2194}' => "<->",
        '\u{21d0}' => "<=",
        '\u{21d2}' => "=>",
        '\u{21d4}' => "<=>",
        '\u{2713}' | '\u{2714}' => "v",
        '\u{2717}' | '\u{2718}' => "x",
        _ => return None,
    })
}
fn emoji_fold(c: char) -> Option<&'static str> {
    Some(match c {
        '✅' | '☑' | '✔' => "[OK]",
        '❌' | '❎' | '✖' => "[FAIL]",
        '⚠' => "[WARN]",
        '🚨' => "[ALERT]",
        'ℹ' => "[INFO]",
        '🐛' => "[BUG]",
        '💥' => "[CRASH]",
        '🔥' => "[HOT]",
        '🔒' => "[LOCK]",
        '🔓' => "[UNLOCK]",
        '📁' | '📂' => "[DIR]",
        '📄' => "[FILE]",
        '📝' => "[NOTE]",
        '🧪' => "[TEST]",
        '⏳' | '⌛' => "[WAIT]",
        '🚀' => "[RUN]",
        _ => return None,
    })
}
fn pictograph(c: char) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    regex_cached(&RE, r"\p{Extended_Pictographic}").is_match(c.encode_utf8(&mut [0; 4]))
}
pub(crate) fn unrenderable(c: char) -> bool {
    matches!(get_general_category(c), Gc::Control | Gc::NonspacingMark | Gc::EnclosingMark | Gc::Surrogate)
}
fn ascii_latin(c: char) -> bool {
    matches!(c as u32, 0x20..=0x7e | 0xa0..=0xff)
}
fn fold_ascii(c: char) -> Option<String> {
    let decomposed: String = c
        .to_string()
        .nfkd()
        .filter(|&part| !matches!(get_general_category(part), Gc::NonspacingMark | Gc::SpacingMark | Gc::EnclosingMark))
        .collect();
    if decomposed == c.to_string() {
        return None;
    }
    let mut out = String::new();
    for part in decomposed.chars() {
        if ascii_latin(part) {
            out.push(part);
        } else {
            out.push_str(char_fold(part)?);
        }
    }
    Some(out)
}
fn strip_ansi(text: &str) -> String {
    // CSI/OSC/DCS/APC/PM/SOS and simple ESC sequences used by Bun.stripANSI.
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            Some(']' | 'P' | '_' | '^' | 'X') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' || c == '\u{1b}' && chars.next_if_eq(&'\\').is_some() {
                        break;
                    }
                }
            }
            Some('(' | ')' | '*' | '+' | '-' | '.' | '/' | '#') => {
                chars.next();
            }
            Some(_) | None => {}
        }
    }
    out
}
pub(crate) fn normalized_input_chars(text: &str) -> Vec<char> {
    let stripped = if text.contains('\u{1b}') { strip_ansi(text) } else { text.into() };
    let mut out = String::new();
    let mut chars = stripped.chars().peekable();
    while let Some(c) = chars.next() {
        if !is_space(c) && get_general_category(c) != Gc::Format {
            out.push(c);
            continue;
        }
        let mut newline = matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}');
        let mut nonformat = get_general_category(c) != Gc::Format;
        while let Some(&c) = chars.peek() {
            if !is_space(c) && get_general_category(c) != Gc::Format {
                break;
            }
            newline |= matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}');
            nonformat |= get_general_category(c) != Gc::Format;
            chars.next();
        }
        if newline {
            out.push(NEWLINE_GLYPH);
        } else if nonformat {
            out.push(' ');
        }
    }
    out.trim_matches([' ', NEWLINE_GLYPH]).chars().collect()
}
fn normalize_stats(text: &str, options: &NormalizeOptions) -> (String, usize, usize) {
    let chars = normalized_input_chars(text);
    let candidates: BTreeSet<_> = chars
        .iter()
        .copied()
        .filter(|&c| {
            !ascii_latin(c)
                && !matches!(c, DIM_ON | DIM_OFF | NEWLINE_GLYPH)
                && char_fold(c).is_none()
                && !(0x2500..=0x257f).contains(&(c as u32))
                && emoji_fold(c).is_none()
                && !pictograph(c)
                && fold_ascii(c).is_none()
                && !unrenderable(c)
        })
        .collect();
    let primary = options.font.as_deref().or_else(|| options.shape.as_ref().map(|s| s.font.as_str())).unwrap_or("5x8");
    let candidate_text: String = candidates.into_iter().collect();
    let mut supported: HashSet<char> =
        crate::native::snapcompact_supported_chars(primary.into(), candidate_text.clone())
            .unwrap_or_default()
            .chars()
            .collect();
    if primary != "silver" {
        supported.extend(
            crate::native::snapcompact_supported_chars("silver".into(), candidate_text).unwrap_or_default().chars(),
        );
    }
    let (mut total, mut fallback) = (0, 0);
    let mut out = String::new();
    for c in chars {
        if ascii_latin(c) {
            out.push(c);
            total += 1;
        } else if matches!(c, DIM_ON | DIM_OFF | NEWLINE_GLYPH) {
            out.push(c);
        } else if let Some(fold) = emoji_fold(c).or_else(|| char_fold(c)) {
            out.push_str(fold);
            total += 1;
        } else if (0x2500..=0x257f).contains(&(c as u32)) {
            out.push(match c as u32 {
                0x2502 | 0x2503 => '|',
                0x2500 | 0x2501 => '-',
                _ => '+',
            });
            total += 1;
        } else if !pictograph(c) && supported.contains(&c) {
            out.push(c);
            total += 1;
        } else if let Some(fold) = fold_ascii(c) {
            out.push_str(&fold);
            total += 1;
        } else if !pictograph(c) && !unrenderable(c) {
            out.push('?');
            total += 1;
            fallback += 1;
        }
    }
    let mut collapsed = String::with_capacity(out.len());
    let mut last_space = false;
    for c in out.chars() {
        if c != ' ' || !last_space {
            collapsed.push(c);
        }
        last_space = c == ' ';
    }
    (collapsed.trim_matches([' ', NEWLINE_GLYPH]).into(), total, fallback)
}
pub fn normalize(text: &str, options: NormalizeOptions) -> String {
    normalize_stats(text, &options).0
}
pub fn scan_renderability(text: &str, options: NormalizeOptions) -> Renderability {
    let (_, total, fallback) = normalize_stats(text, &options);
    let ratio = if total > 0 { fallback as f64 / total as f64 } else { 0.0 };
    Renderability { is_safe: ratio <= 0.05, unrenderable_ratio: ratio }
}
pub fn dim_stopwords(text: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    const WORDS: &str = "the a an and or of to in on at as is are was were be been by for with that this it its from had has have not but he she his her they their them which also who whom when where while will would could should there then than into over under about after before between during each such these those some most more other only same so";
    let re = regex_cached(&RE, r"[a-zA-Z\x{c0}-\x{d6}\x{d8}-\x{f6}\x{f8}-\x{ff}]+");
    let mut dim = false;
    let mut out = String::new();
    let mut start = 0;
    for (at, c) in text.char_indices().filter(|(_, c)| matches!(c, &DIM_ON | &DIM_OFF)) {
        let part = &text[start..at];
        if dim {
            out.push_str(part);
        } else {
            out.push_str(&re.replace_all(part, |caps: &regex::Captures<'_>| {
                let word = &caps[0];
                if WORDS.split(' ').any(|w| w == word.to_lowercase()) {
                    format!("{DIM_ON}{word}{DIM_OFF}")
                } else {
                    word.into()
                }
            }));
        }
        out.push(c);
        dim = c == DIM_ON;
        start = at + c.len_utf8();
    }
    let part = &text[start..];
    if dim {
        out.push_str(part);
    } else {
        out.push_str(&re.replace_all(part, |caps: &regex::Captures<'_>| {
            let word = &caps[0];
            if WORDS.split(' ').any(|w| w == word.to_lowercase()) {
                format!("{DIM_ON}{word}{DIM_OFF}")
            } else {
                word.into()
            }
        }));
    }
    out
}
