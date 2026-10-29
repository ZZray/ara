//! Fixed OMP transcript serializer, atomic data-URL healing and font-aware folds.
use crate::utf16::{Utf16CodePoint, Utf16Text};
use crate::{DIM_OFF, DIM_ON, NEWLINE_GLYPH, NormalizeOptions, Renderability, SerializeOptions};
use ara_prompt::js::{is_space, utf16_len};
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
    strip_dim_markers_utf16(&Utf16Text::from(text)).to_lossy_string()
}
fn strip_dim_markers_utf16(text: &Utf16Text) -> Utf16Text {
    Utf16Text::from_units(
        text.as_units().iter().copied().filter(|&u| u != DIM_ON as u16 && u != DIM_OFF as u16).collect(),
    )
}
pub(crate) fn to_plain_text(text: &str) -> String {
    to_plain_text_utf16(&Utf16Text::from(text)).to_lossy_string()
}
pub(crate) fn to_plain_text_utf16(text: &Utf16Text) -> Utf16Text {
    Utf16Text::from_units(
        strip_dim_markers_utf16(text)
            .as_units()
            .iter()
            .map(|&unit| if unit == NEWLINE_GLYPH as u16 { b'\n' as u16 } else { unit })
            .collect(),
    )
}

// Regexes below only classify ASCII syntax, BMP whitespace and Latin words.
// A lone unit is an opaque nonmatching character in this temporary matcher;
// every untouched output span is copied from the original UTF16 units.
struct MatchText {
    text: String,
    boundaries: Vec<(usize, usize)>,
}
impl MatchText {
    fn new(text: &Utf16Text) -> Self {
        let mut out = String::new();
        let mut boundaries = Vec::new();
        let mut units = 0;
        for point in text.codepoints() {
            boundaries.push((out.len(), units));
            out.push(match point {
                Utf16CodePoint::Scalar(c) => c,
                Utf16CodePoint::Lone(_) => '\u{fffd}',
            });
            units += point.len_utf16();
        }
        boundaries.push((out.len(), units));
        Self { text: out, boundaries }
    }
    fn unit_offset(&self, byte: usize) -> usize {
        let index =
            self.boundaries.binary_search_by_key(&byte, |&(at, _)| at).expect("regex boundary is a codepoint boundary");
        self.boundaries[index].1
    }
    fn slice(&self, source: &Utf16Text, start: usize, end: usize) -> Utf16Text {
        source.slice(self.unit_offset(start), self.unit_offset(end))
    }
}
fn truncate(text: &Utf16Text, cap: f64, head_ratio: f64) -> Utf16Text {
    let len = text.len();
    if len as f64 <= cap {
        return text.clone();
    }
    let head = (cap * head_ratio.clamp(0.0, 1.0)).round() as usize;
    let tail = (cap - head as f64).max(0.0) as usize;
    let mut out = text.slice(0, head);
    out.push_str(&format!(" […{}ch elided…] ", ara_prompt::js::f64_to_string(len as f64 - cap)));
    if tail > 0 {
        out.append(&text.slice(len.saturating_sub(tail), len));
    }
    out
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
    elide_data_urls_utf16(&Utf16Text::from(text), archive).to_lossy_string()
}
pub(crate) fn elide_data_urls_utf16(source: &Utf16Text, archive: bool) -> Utf16Text {
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
    let matched = MatchText::new(source);
    let text = matched.text.as_str();
    let mut out = Utf16Text::default();
    let mut cursor = 0;
    for captures in atoms.captures_iter(text) {
        let atom = captures.get(0).expect("full regex match");
        let payload = captures.get(2).map(|m| m.as_str()).unwrap_or("");
        let marker = marker_re.find(payload);
        if !archive && marker.is_none() && !canonical.is_match(payload) && utf16_len(payload) < 40 {
            out.append(&matched.slice(source, cursor, atom.end()));
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
        out.append(&matched.slice(source, cursor, opener.unwrap_or(atom.start())));
        if opener.is_some() && closer.is_some() {
            out.push_str(&placeholder);
        } else {
            if let Some(start) = opener {
                out.append(&matched.slice(source, start, atom.start()));
            }
            out.push_str(&placeholder);
            if let Some(closer) = closer {
                out.push_str(closer);
            }
        }
        cursor = atom.end();
    }
    out.append(&matched.slice(source, cursor, text.len()));
    out
}

pub(crate) fn strip_thinking_utf16(source: &Utf16Text) -> Utf16Text {
    let mut starts = vec![0];
    starts.extend(section_starts_utf16(source).into_iter().filter(|&at| at > 0));
    starts.push(source.len());
    let mut sections: Vec<Utf16Text> = Vec::new();
    let mut dim_open = false;
    for bounds in starts.windows(2) {
        let piece = source.slice(bounds[0], bounds[1]);
        if dim_open && !sections.is_empty() {
            sections.last_mut().expect("prior section").append(&piece);
        } else {
            sections.push(piece.clone());
        }
        let on = piece.as_units().iter().rposition(|&unit| unit == DIM_ON as u16);
        let off = piece.as_units().iter().rposition(|&unit| unit == DIM_OFF as u16);
        if on != off {
            dim_open = on > off;
        }
    }
    let thinking = |section: &Utf16Text| section.as_units().starts_with(&[182, 116, 104, 105, 110, 107, 58]);
    if !sections.iter().any(thinking) {
        return source.clone();
    }
    let final_thinking = sections.last().is_some_and(thinking);
    let mut kept = Utf16Text::join(&sections.into_iter().filter(|part| !thinking(part)).collect::<Vec<_>>(), "");
    if final_thinking {
        let end = kept.as_units().iter().rposition(|&u| u != 10 && u != NEWLINE_GLYPH as u16).map_or(0, |i| i + 1);
        kept = kept.slice(0, end);
    }
    kept
}

/// Start-of-line scope boundaries, in UTF16 offsets, for raw and normalized text.
pub(crate) fn section_starts_utf16(source: &Utf16Text) -> Vec<usize> {
    let labels: Vec<Vec<u16>> =
        ["¶user:", "¶think:", "¶ai:", "¶call:"].iter().map(|label| label.encode_utf16().collect()).collect();
    let units = source.as_units();
    (0..units.len())
        .filter(|&at| {
            (at == 0 || units[at - 1] == 10 || units[at - 1] == NEWLINE_GLYPH as u16)
                && labels.iter().any(|label| units[at..].starts_with(label))
        })
        .collect()
}

pub fn summary_includes_thinking(summary: &str) -> bool {
    summary.contains("¶think:")
}

/// Remove complete reasoning scopes and both historical reasoning legends.
/// Untouched ranges retain escaped lone UTF16 units.
pub fn text_without_thinking_utf16(source: &Utf16Text) -> Utf16Text {
    let stripped = strip_thinking_utf16(source);
    let matched = MatchText::new(&stripped);
    static LEGEND: OnceLock<Regex> = OnceLock::new();
    let re = regex_cached(
        &LEGEND,
        r"`¶user:`, `¶think:`, (`¶ai:`, (?:and )?`¶call:`:? (?:open )?user, )assistant reasoning, ",
    );
    let mut out = Utf16Text::default();
    let mut cursor = 0;
    for captures in re.captures_iter(&matched.text) {
        let all = captures.get(0).expect("legend");
        let kept = captures.get(1).expect("remaining scopes");
        out.append(&matched.slice(&stripped, cursor, all.start()));
        out.push_str("`¶user:`, ");
        out.append(&matched.slice(&stripped, kept.start(), kept.end()));
        cursor = all.end();
    }
    out.append(&matched.slice(&stripped, cursor, matched.text.len()));
    out
}

pub fn text_without_thinking(text: &str) -> String {
    text_without_thinking_utf16(&Utf16Text::from(text)).to_scalar().expect("scalar input remains scalar")
}
fn raw_field(value: &serde_json::value::RawValue, key: &str) -> crate::Result<Box<serde_json::value::RawValue>> {
    Ok(crate::raw_json::field(value, key)?.unwrap_or(crate::raw_json::raw("null")?))
}
fn raw_text(value: &serde_json::value::RawValue) -> crate::Result<Utf16Text> {
    Ok(crate::raw_json::decode_string(value)?.unwrap_or_default())
}
fn raw_label(value: &serde_json::value::RawValue) -> crate::Result<String> {
    Ok(raw_text(value)?.to_scalar().unwrap_or_default())
}
fn has_nonspace(text: &Utf16Text) -> bool {
    text.codepoints().any(|point| !matches!(point, Utf16CodePoint::Scalar(c) if is_space(c)))
}
fn intent_text(text: &Utf16Text) -> Utf16Text {
    let parts: Vec<_> = text
        .as_units()
        .split(|&u| char::from_u32(u32::from(u)).is_some_and(is_space))
        .filter(|s| !s.is_empty())
        .map(|s| Utf16Text::from_units(s.to_vec()))
        .collect();
    Utf16Text::join(&parts, " ")
}

// This bounded input view covers only the message fields consumed by fixed
// OMP's serializer. Raw JSON ownership remains in the caller's facade.
struct SerializeMessage {
    role: String,
    id: Utf16Text,
    useless: bool,
    error: bool,
    content: Utf16Text,
    blocks: Vec<SerializeBlock>,
}
enum SerializeBlock {
    Text(Utf16Text),
    Thinking(Utf16Text),
    Call { id: Utf16Text, name: Utf16Text, intent: Utf16Text, args: Vec<(Utf16Text, Utf16Text)> },
}
fn read_content(value: &serde_json::value::RawValue) -> crate::Result<Utf16Text> {
    if let Some(text) = crate::raw_json::decode_string(value)? {
        return Ok(text);
    }
    let mut text = Utf16Text::default();
    for block in crate::raw_json::array_values(value)?.unwrap_or_default() {
        if raw_label(&raw_field(&block, "type")?)? == "text" {
            text.append(&raw_text(&raw_field(&block, "text")?)?);
        }
    }
    Ok(text)
}
fn read_message(value: &serde_json::value::RawValue) -> crate::Result<SerializeMessage> {
    let role = raw_label(&raw_field(value, "role")?)?;
    let content = raw_field(value, "content")?;
    let mut blocks = Vec::new();
    if role == "assistant" {
        for block in crate::raw_json::array_values(&content)?.unwrap_or_default() {
            match raw_label(&raw_field(&block, "type")?)?.as_str() {
                "text" => blocks.push(SerializeBlock::Text(raw_text(&raw_field(&block, "text")?)?)),
                "thinking" => blocks.push(SerializeBlock::Thinking(raw_text(&raw_field(&block, "thinking")?)?)),
                "toolCall" => {
                    let arguments = raw_field(&block, "arguments")?;
                    let intent = crate::raw_json::decode_string(&raw_field(&block, "intent")?)?
                        .unwrap_or(raw_text(&raw_field(&arguments, "i")?)?);
                    let args = crate::raw_json::entries(&arguments)?
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|member| member.key.as_units() != [b'i' as u16])
                        .map(|member| Ok((member.key, crate::raw_json::stringify(&member.value)?)))
                        .collect::<crate::Result<Vec<_>>>()?;
                    blocks.push(SerializeBlock::Call {
                        id: raw_text(&raw_field(&block, "id")?)?,
                        name: raw_text(&raw_field(&block, "name")?)?,
                        intent,
                        args,
                    });
                }
                _ => {}
            }
        }
    }
    Ok(SerializeMessage {
        role,
        id: raw_text(&raw_field(value, "toolCallId")?)?,
        useless: raw_field(value, "useless")?.get().trim() == "true",
        error: raw_field(value, "isError")?.get().trim() == "true",
        content: read_content(&content)?,
        blocks,
    })
}
fn push_part(parts: &mut Vec<Utf16Text>, last: &mut String, prefix: &str, text: &Utf16Text) {
    if last == prefix
        && let Some(part) = parts.last_mut()
    {
        if part.as_units().last() != Some(&(b'\n' as u16)) && text.as_units().first() != Some(&(b'\n' as u16)) {
            part.push_str("\n");
        }
        part.append(text);
    } else {
        let mut part = Utf16Text::from(prefix);
        part.append(text);
        parts.push(part);
        *last = prefix.into();
    }
}
fn flush_assistant(
    parts: &mut Vec<Utf16Text>,
    last: &mut String,
    thinking: &mut Vec<Utf16Text>,
    text: &mut Vec<Utf16Text>,
) {
    if !thinking.is_empty() {
        push_part(parts, last, "¶think:", &Utf16Text::join(thinking, "\n"));
        thinking.clear();
    }
    if !text.is_empty() {
        push_part(parts, last, "¶ai:", &Utf16Text::join(text, "\n"));
        text.clear();
    }
}
fn serialize_messages(messages: &[SerializeMessage], options: &SerializeOptions) -> Utf16Text {
    let mut useless = HashSet::new();
    let mut results = HashMap::new();
    for message in messages {
        if message.role != "toolResult" {
            continue;
        }
        if message.useless && !message.error {
            useless.insert(&message.id);
            continue;
        }
        if !message.content.is_empty() {
            results.insert(&message.id, &message.content);
        }
    }
    let result_block = |text: &Utf16Text| {
        let body = truncate(
            &elide_data_urls_utf16(&strip_dim_markers_utf16(text), false),
            options.tool_result_max_chars,
            options.truncate_head_ratio,
        );
        let mut out = Utf16Text::from("<out>\n");
        if options.dim_tool_results {
            out.push_str(&DIM_ON.to_string());
        }
        out.append(&body);
        if options.dim_tool_results {
            out.push_str(&DIM_OFF.to_string());
        }
        out.push_str("\n</out>");
        out
    };
    let mut merged = HashSet::new();
    let mut parts = Vec::new();
    let mut last = String::new();
    for message in messages {
        match message.role.as_str() {
            "user" if !message.content.is_empty() => {
                push_part(&mut parts, &mut last, "¶user:", &strip_dim_markers_utf16(&message.content))
            }
            "assistant" => {
                let mut thinking = Vec::new();
                let mut text = Vec::new();
                for block in &message.blocks {
                    match block {
                        SerializeBlock::Text(value) => {
                            let value = strip_dim_markers_utf16(value);
                            if has_nonspace(&value) {
                                text.push(value);
                            }
                        }
                        SerializeBlock::Thinking(value) if options.include_thinking => {
                            let value = strip_dim_markers_utf16(value);
                            if has_nonspace(&value) {
                                thinking.push(value);
                            }
                        }
                        SerializeBlock::Call { id, name, intent, args } => {
                            if useless.contains(id) {
                                continue;
                            }
                            flush_assistant(&mut parts, &mut last, &mut thinking, &mut text);
                            let intent = intent_text(&strip_dim_markers_utf16(intent));
                            let args: Vec<_> = args
                                .iter()
                                .map(|(key, value)| {
                                    let mut arg = key.clone();
                                    arg.push_str("=");
                                    arg.append(&truncate(
                                        &elide_data_urls_utf16(value, false),
                                        options.tool_arg_max_chars,
                                        options.truncate_head_ratio,
                                    ));
                                    arg
                                })
                                .collect();
                            let args = truncate(
                                &Utf16Text::join(&args, ", "),
                                options.tool_call_max_chars,
                                options.truncate_head_ratio,
                            );
                            let mut call = name.clone();
                            call.push_str("(");
                            call.append(&args);
                            call.push_str(")");
                            if !intent.is_empty() {
                                call.push_str("//");
                                call.append(&intent);
                            }
                            if let Some(result) = results.get(id) {
                                merged.insert(id);
                                call.push_str("\n");
                                call.append(&result_block(result));
                            }
                            push_part(&mut parts, &mut last, "¶call:", &call);
                        }
                        _ => {}
                    }
                }
                flush_assistant(&mut parts, &mut last, &mut thinking, &mut text);
            }
            "toolResult" => {
                if !useless.contains(&message.id)
                    && !merged.contains(&message.id)
                    && let Some(result) = results.get(&message.id)
                {
                    let mut content = Utf16Text::from("\n");
                    content.append(&result_block(result));
                    push_part(&mut parts, &mut last, "¶call:", &content);
                }
            }
            _ => {}
        }
    }
    Utf16Text::join(&parts, "\n\n")
}

/// Exact message facade. Escaped lone surrogates survive both input parsing and
/// each serializer truncation stage; the output remains UTF16 units.
pub fn serialize_conversation_raw_utf16(
    messages: &[Box<serde_json::value::RawValue>],
    options: &SerializeOptions,
) -> crate::Result<Utf16Text> {
    let messages = messages.iter().map(|message| read_message(message)).collect::<crate::Result<Vec<_>>>()?;
    Ok(serialize_messages(&messages, options))
}

/// Scalar message convenience facade into the same bounded parser and kernel.
pub fn serialize_conversation_utf16(messages: &[Value], options: &SerializeOptions) -> Utf16Text {
    let messages: Vec<_> = messages
        .iter()
        .map(|message| serde_json::value::to_raw_value(message).expect("Value serializes as valid raw JSON"))
        .collect();
    serialize_conversation_raw_utf16(&messages, options).expect("Scalar message view is valid JSON")
}

/// Explicit scalar-output compatibility projection. Exact archive consumers use
/// the RawValue or UTF16 output facade before normalization or archival slicing.
pub fn serialize_conversation(messages: &[Value], options: &SerializeOptions) -> String {
    serialize_conversation_utf16(messages, options).to_lossy_string()
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
fn points_to_text(points: &[Utf16CodePoint]) -> Utf16Text {
    let mut units = Vec::new();
    for &point in points {
        match point {
            Utf16CodePoint::Scalar(c) => units.extend_from_slice(c.encode_utf16(&mut [0; 2])),
            Utf16CodePoint::Lone(unit) => units.push(unit),
        }
    }
    Utf16Text::from_units(units)
}
fn strip_ansi(text: &Utf16Text) -> Utf16Text {
    use Utf16CodePoint::Scalar;
    // The existing CSI/OSC/DCS/APC/PM/SOS parser now keeps opaque lone units.
    let mut out = Vec::new();
    let mut chars = text.codepoints().peekable();
    while let Some(c) = chars.next() {
        if c != Scalar('\u{1b}') {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some(Scalar('[')) => {
                for c in chars.by_ref() {
                    if let Scalar(c) = c
                        && ('@'..='~').contains(&c)
                    {
                        break;
                    }
                }
            }
            Some(Scalar(']' | 'P' | '_' | '^' | 'X')) => {
                while let Some(c) = chars.next() {
                    if c == Scalar('\u{7}') || c == Scalar('\u{1b}') && chars.next_if_eq(&Scalar('\\')).is_some() {
                        break;
                    }
                }
            }
            Some(Scalar('(' | ')' | '*' | '+' | '-' | '.' | '/' | '#')) => {
                chars.next();
            }
            Some(_) | None => {}
        }
    }
    // Removing ANSI can join formerly separate surrogate units. JavaScript
    // subsequently applies its Unicode whitespace regex to this new string.
    points_to_text(&out)
}
fn collapsible(c: Utf16CodePoint) -> bool {
    matches!(c, Utf16CodePoint::Scalar(c) if is_space(c) || get_general_category(c) == Gc::Format)
}
pub(crate) fn normalized_input_utf16(text: &Utf16Text) -> Vec<Utf16CodePoint> {
    use Utf16CodePoint::Scalar;
    let stripped = if text.as_units().contains(&0x1b) { strip_ansi(text) } else { text.clone() };
    let mut out = Vec::new();
    let mut chars = stripped.codepoints().peekable();
    while let Some(c) = chars.next() {
        if !collapsible(c) {
            out.push(c);
            continue;
        }
        let mut newline = false;
        let mut nonformat = false;
        let mut observe = |point| {
            if let Scalar(c) = point {
                newline |= matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}');
                nonformat |= get_general_category(c) != Gc::Format;
            }
        };
        observe(c);
        while let Some(&c) = chars.peek() {
            if !collapsible(c) {
                break;
            }
            observe(c);
            chars.next();
        }
        if newline {
            out.push(Scalar(NEWLINE_GLYPH));
        } else if nonformat {
            out.push(Scalar(' '));
        }
    }
    let edge = |c: &Utf16CodePoint| matches!(c, Scalar(' ') | Scalar(NEWLINE_GLYPH));
    let start = out.iter().position(|c| !edge(c)).unwrap_or(out.len());
    let end = out.iter().rposition(|c| !edge(c)).map_or(start, |index| index + 1);
    // Pure Cf deletion can also join surrogate units. Source spreads the
    // collapsed string only now, so re-decode before the later Cs fold.
    points_to_text(&out[start..end]).codepoints().collect()
}
pub(crate) fn normalized_input_chars(text: &str) -> Vec<char> {
    normalized_input_utf16(&Utf16Text::from(text))
        .into_iter()
        .filter_map(|c| match c {
            Utf16CodePoint::Scalar(c) => Some(c),
            Utf16CodePoint::Lone(_) => None,
        })
        .collect()
}
fn normalize_stats(text: &Utf16Text, options: &NormalizeOptions) -> (String, usize, usize) {
    // Source normalizedInputChars first strips ANSI and collapses whitespace.
    // Only its later Cs fold drops lone units; they remain run barriers above.
    let chars: Vec<char> = normalized_input_utf16(text)
        .into_iter()
        .filter_map(|c| match c {
            Utf16CodePoint::Scalar(c) => Some(c),
            Utf16CodePoint::Lone(_) => None,
        })
        .collect();
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
pub fn normalize_utf16(text: &Utf16Text, options: NormalizeOptions) -> String {
    normalize_stats(text, &options).0
}
pub fn normalize(text: &str, options: NormalizeOptions) -> String {
    normalize_utf16(&Utf16Text::from(text), options)
}
pub fn scan_renderability_utf16(text: &Utf16Text, options: NormalizeOptions) -> Renderability {
    let (_, total, fallback) = normalize_stats(text, &options);
    let ratio = if total > 0 { fallback as f64 / total as f64 } else { 0.0 };
    Renderability { is_safe: ratio <= 0.05, unrenderable_ratio: ratio }
}
pub fn scan_renderability(text: &str, options: NormalizeOptions) -> Renderability {
    scan_renderability_utf16(&Utf16Text::from(text), options)
}
pub fn dim_stopwords(text: &str) -> String {
    dim_stopwords_utf16(&Utf16Text::from(text)).to_lossy_string()
}
pub(crate) fn dim_stopwords_utf16(source: &Utf16Text) -> Utf16Text {
    static RE: OnceLock<Regex> = OnceLock::new();
    const WORDS: &str = "the a an and or of to in on at as is are was were be been by for with that this it its from had has have not but he she his her they their them which also who whom when where while will would could should there then than into over under about after before between during each such these those some most more other only same so";
    let re = regex_cached(&RE, r"[a-zA-Z\x{c0}-\x{d6}\x{d8}-\x{f6}\x{f8}-\x{ff}]+");
    let mut dim = false;
    let matched = MatchText::new(source);
    let text = matched.text.as_str();
    let mut out = Utf16Text::default();
    let mut start = 0;
    for (at, c) in text.char_indices().filter(|(_, c)| matches!(c, &DIM_ON | &DIM_OFF)) {
        let part = &text[start..at];
        if dim {
            out.append(&matched.slice(source, start, at));
        } else {
            let mut cursor = start;
            for found in re.find_iter(part) {
                out.append(&matched.slice(source, cursor, start + found.start()));
                let word = found.as_str();
                if WORDS.split(' ').any(|w| w == word.to_lowercase()) {
                    out.push_str(&format!("{DIM_ON}{word}{DIM_OFF}"));
                } else {
                    out.append(&matched.slice(source, start + found.start(), start + found.end()));
                }
                cursor = start + found.end();
            }
            out.append(&matched.slice(source, cursor, at));
        }
        out.push_str(&c.to_string());
        dim = c == DIM_ON;
        start = at + c.len_utf8();
    }
    let part = &text[start..];
    if dim {
        out.append(&matched.slice(source, start, text.len()));
    } else {
        let mut cursor = start;
        for found in re.find_iter(part) {
            out.append(&matched.slice(source, cursor, start + found.start()));
            let word = found.as_str();
            if WORDS.split(' ').any(|w| w == word.to_lowercase()) {
                out.push_str(&format!("{DIM_ON}{word}{DIM_OFF}"));
            } else {
                out.append(&matched.slice(source, start + found.start(), start + found.end()));
            }
            cursor = start + found.end();
        }
        out.append(&matched.slice(source, cursor, text.len()));
    }
    out
}
