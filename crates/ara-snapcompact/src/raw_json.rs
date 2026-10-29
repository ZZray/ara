//! Bounded JSON span access for snapcompact messages and persisted archives.
//! RawValue validates JSON syntax; these helpers retain string UTF16 units and
//! untouched member spans. They implement no executable JavaScript behavior.
use crate::{Error, Result, Utf16Text};
use serde_json::value::RawValue;

pub(crate) struct RawMember {
    pub key: Utf16Text,
    pub key_json: String,
    pub value: Box<RawValue>,
}
pub(crate) fn raw(source: &str) -> Result<Box<RawValue>> {
    RawValue::from_string(source.to_owned()).map_err(|e| Error::from_reason(e.to_string()))
}
fn skip_space(bytes: &[u8], at: &mut usize) {
    while bytes.get(*at).is_some_and(u8::is_ascii_whitespace) {
        *at += 1;
    }
}
fn string_end(bytes: &[u8], start: usize) -> usize {
    let mut at = start + 1;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => at += 2,
            b'"' => return at + 1,
            _ => at += 1,
        }
    }
    bytes.len()
}
fn value_end(bytes: &[u8], start: usize) -> usize {
    if bytes[start] == b'"' {
        return string_end(bytes, start);
    }
    if matches!(bytes[start], b'{' | b'[') {
        let mut depth = 0;
        let mut at = start;
        while at < bytes.len() {
            match bytes[at] {
                b'"' => {
                    at = string_end(bytes, at);
                    continue;
                }
                b'{' | b'[' => depth += 1,
                b'}' | b']' => {
                    depth -= 1;
                    if depth == 0 {
                        return at + 1;
                    }
                }
                _ => {}
            }
            at += 1;
        }
        return bytes.len();
    }
    let mut at = start;
    while at < bytes.len() && !matches!(bytes[at], b',' | b'}' | b']') && !bytes[at].is_ascii_whitespace() {
        at += 1;
    }
    at
}
pub(crate) fn object_members(value: &RawValue) -> Result<Option<Vec<RawMember>>> {
    let source = value.get().trim();
    if !source.starts_with('{') {
        return Ok(None);
    }
    let bytes = source.as_bytes();
    let mut at = 1;
    let mut members = Vec::new();
    loop {
        skip_space(bytes, &mut at);
        if bytes[at] == b'}' {
            break;
        }
        let end = string_end(bytes, at);
        let key_json = source[at..end].to_owned();
        let key = decode_string(&raw(&key_json)?)?.expect("validated object key");
        at = end;
        skip_space(bytes, &mut at);
        at += 1; // validated colon
        skip_space(bytes, &mut at);
        let end = value_end(bytes, at);
        members.push(RawMember { key, key_json, value: raw(&source[at..end])? });
        at = end;
        skip_space(bytes, &mut at);
        if bytes[at] == b'}' {
            break;
        }
        at += 1; // validated comma
    }
    Ok(Some(members))
}
pub(crate) fn array_values(value: &RawValue) -> Result<Option<Vec<Box<RawValue>>>> {
    let source = value.get().trim();
    if !source.starts_with('[') {
        return Ok(None);
    }
    let bytes = source.as_bytes();
    let mut at = 1;
    let mut values = Vec::new();
    loop {
        skip_space(bytes, &mut at);
        if bytes[at] == b']' {
            break;
        }
        let end = value_end(bytes, at);
        values.push(raw(&source[at..end])?);
        at = end;
        skip_space(bytes, &mut at);
        if bytes[at] == b']' {
            break;
        }
        at += 1;
    }
    Ok(Some(values))
}
pub(crate) fn field(value: &RawValue, name: &str) -> Result<Option<Box<RawValue>>> {
    let key = Utf16Text::from(name);
    Ok(object_members(value)?.and_then(|members| members.into_iter().rev().find(|m| m.key == key).map(|m| m.value)))
}
pub(crate) fn decode_string(value: &RawValue) -> Result<Option<Utf16Text>> {
    let source = value.get().trim();
    if !source.starts_with('"') {
        return Ok(None);
    }
    let mut units = Vec::new();
    let mut at = 1;
    let bytes = source.as_bytes();
    while at + 1 < source.len() {
        if bytes[at] != b'\\' {
            let c = source[at..].chars().next().expect("validated UTF8");
            units.extend(c.encode_utf16(&mut [0; 2]).iter().copied());
            at += c.len_utf8();
            continue;
        }
        at += 1;
        match bytes[at] {
            b'u' => {
                units.push(u16::from_str_radix(&source[at + 1..at + 5], 16).expect("validated JSON escape"));
                at += 5;
            }
            escaped => {
                units.push(match escaped {
                    b'b' => 8,
                    b'f' => 12,
                    b'n' => 10,
                    b'r' => 13,
                    b't' => 9,
                    _ => escaped as u16,
                });
                at += 1;
            }
        }
    }
    Ok(Some(Utf16Text::from_units(units)))
}
pub(crate) fn encode_string(text: &Utf16Text) -> String {
    use std::fmt::Write;
    let mut json = String::from("\"");
    for point in text.codepoints() {
        match point {
            crate::Utf16CodePoint::Lone(unit) => {
                write!(&mut json, "\\u{unit:04x}").expect("String write");
            }
            crate::Utf16CodePoint::Scalar(c) => match c {
                '"' => json.push_str("\\\""),
                '\\' => json.push_str("\\\\"),
                '\u{8}' => json.push_str("\\b"),
                '\u{c}' => json.push_str("\\f"),
                '\n' => json.push_str("\\n"),
                '\r' => json.push_str("\\r"),
                '\t' => json.push_str("\\t"),
                c if c <= '\u{1f}' => {
                    write!(&mut json, "\\u{:04x}", c as u32).expect("String write");
                }
                c => json.push(c),
            },
        }
    }
    json.push('"');
    json
}

/// JSON object entry semantics only: later duplicates replace the value in
/// its first insertion slot; array-index keys precede the remaining keys.
pub(crate) fn entries(value: &RawValue) -> Result<Option<Vec<RawMember>>> {
    let mut unique: Vec<RawMember> = Vec::new();
    let Some(members) = object_members(value)? else {
        return Ok(None);
    };
    for member in members {
        if let Some(existing) = unique.iter_mut().find(|m| m.key == member.key) {
            existing.value = member.value;
        } else {
            unique.push(member);
        }
    }
    fn index(key: &Utf16Text) -> Option<u32> {
        let key = key.to_scalar().ok()?;
        let number = key.parse::<u32>().ok()?;
        (number != u32::MAX && number.to_string() == key).then_some(number)
    }
    unique.sort_by_key(|m| (index(&m.key).is_none(), index(&m.key).unwrap_or(0)));
    Ok(Some(unique))
}

/// Restricted JSON.stringify for JSON tool arguments; there are no undefined,
/// executable objects, toJSON hooks, or JavaScript runtime values here.
pub(crate) fn stringify(value: &RawValue) -> Result<Utf16Text> {
    enum Work {
        Value(Box<RawValue>),
        Text(String),
    }
    let mut work = vec![Work::Value(raw(value.get())?)];
    let mut out = Utf16Text::default();
    while let Some(item) = work.pop() {
        let value = match item {
            Work::Text(text) => {
                out.push_str(&text);
                continue;
            }
            Work::Value(value) => value,
        };
        if let Some(text) = decode_string(&value)? {
            out.push_str(&encode_string(&text));
        } else if let Some(values) = array_values(&value)? {
            out.push_str("[");
            work.push(Work::Text("]".into()));
            for (index, value) in values.into_iter().enumerate().rev() {
                work.push(Work::Value(value));
                if index > 0 {
                    work.push(Work::Text(",".into()));
                }
            }
        } else if let Some(members) = entries(&value)? {
            out.push_str("{");
            work.push(Work::Text("}".into()));
            for (index, member) in members.into_iter().enumerate().rev() {
                work.push(Work::Value(member.value));
                work.push(Work::Text(format!("{}:", encode_string(&member.key))));
                if index > 0 {
                    work.push(Work::Text(",".into()));
                }
            }
        } else {
            let source = value.get().trim();
            if matches!(source, "null" | "true" | "false") {
                out.push_str(source);
            } else {
                let number = source.parse::<f64>().map_err(|e| Error::from_reason(e.to_string()))?;
                out.push_str(&if number.is_finite() { ara_prompt::js::f64_to_string(number) } else { "null".into() });
            }
        }
    }
    Ok(out)
}
