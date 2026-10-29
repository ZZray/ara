//! Fixed OMP write.ts:708-827 and sqlite-reader.ts write helpers (MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See LICENSE.
//!
//! Existing databases only. The caller owns policy, progress, admission and
//! filesystem cache invalidation. SQL is synchronous, as in bun:sqlite; no
//! cancellation/retry worker can abandon a potentially completed mutation.

use ara_agent::ToolError;
use ara_prompt::js::{is_space, trim};
use ara_rpc::WireString;
use rusqlite::types::{ToSql, ToSqlOutput, ValueRef};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params_from_iter};
use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use std::fmt;
use std::io::Read;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::Duration;

pub(crate) struct Target {
    pub absolute: PathBuf,
    pub database_path: String,
    table: String,
    key: Option<String>,
    exists: bool,
}

fn tool_error(error: impl fmt::Display) -> ToolError {
    ToolError(error.to_string())
}

fn target_table(remainder: &str) -> Result<(String, Option<String>), ToolError> {
    let (path, query) = remainder.split_once('?').unwrap_or((remainder, ""));
    if !trim(query).is_empty() {
        return Err(ToolError("SQLite write paths do not support query parameters".into()));
    }
    let path = trim(path.trim_start_matches(':'));
    if path.is_empty() {
        return Err(ToolError("SQLite write path must target a table".into()));
    }
    let (table, key) = match path.split_once(':') {
        Some((table, key)) => (table, Some(key)),
        None => (path, None),
    };
    if table.is_empty() {
        return Err(ToolError("SQLite write path must target a table".into()));
    }
    if key.is_some_and(str::is_empty) {
        return Err(ToolError("SQLite row writes require a non-empty row key".into()));
    }
    Ok((table.to_owned(), key.map(str::to_owned)))
}

fn has_magic(path: &std::path::Path) -> bool {
    let mut bytes = [0u8; 16];
    std::fs::File::open(path).and_then(|mut file| file.read_exact(&mut bytes)).is_ok() && &bytes == b"SQLite format 3\0"
}

pub(crate) fn resolve(
    path: &str,
    resolve_path: impl Fn(&str) -> Result<PathBuf, ToolError>,
) -> Result<Option<Target>, ToolError> {
    static PATTERN: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"(?i)\.(?:sqlite3?|db3?)").unwrap());
    let normalized = path.replace('\\', "/");
    let mut candidates: Vec<_> = PATTERN
        .find_iter(&normalized)
        .filter(|found| normalized.as_bytes().get(found.end()).is_none_or(|next| matches!(next, b':' | b'?')))
        .filter(|found| found.end() != path.len())
        .map(|found| (&path[..found.end()], &normalized[found.end()..]))
        .collect();
    candidates.sort_by_key(|(database, _)| std::cmp::Reverse(database.len()));
    let Some(&(fallback_path, fallback_remainder)) = candidates.last() else {
        return Ok(None);
    };
    // Fixed owner parses the smallest fallback before probing any candidate.
    let (table, key) = target_table(fallback_remainder)?;
    let fallback = Target {
        absolute: resolve_path(fallback_path)?,
        database_path: fallback_path.to_owned(),
        table,
        key,
        exists: false,
    };
    let mut existing_non_sqlite = false;
    for (database_path, remainder) in candidates {
        let (table, key) = target_table(remainder)?;
        let absolute = resolve_path(database_path)?;
        match std::fs::metadata(&absolute) {
            Ok(meta) if meta.is_dir() => continue,
            Ok(_) if !has_magic(&absolute) => {
                existing_non_sqlite = true;
                continue;
            }
            Ok(_) => {
                return Ok(Some(Target {
                    absolute,
                    database_path: database_path.to_owned(),
                    table,
                    key,
                    exists: true,
                }));
            }
            Err(error) if matches!(error.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory) => {}
            Err(error) => return Err(tool_error(error)),
        }
    }
    Ok(if existing_non_sqlite { None } else { Some(fallback) })
}

// The JSON5 crate validates object/array/number grammar. String tokens are
// decoded separately because Rust String cannot preserve a lone UTF-16 unit.
// All quoted literals become private NUL-prefixed tokens; valid bare keys
// cannot spell those tokens, and user quoted strings are replaced as well.
#[derive(Debug)]
enum Carrier {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Wire(WireString),
    Array(Vec<Carrier>),
    Object(Vec<(String, Carrier)>),
}
impl<'de> Deserialize<'de> for Carrier {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ValueVisitor;
        impl<'de> Visitor<'de> for ValueVisitor {
            type Value = Carrier;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON5 value")
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Carrier, E> {
                Ok(Carrier::Null)
            }
            fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Carrier, E> {
                Ok(Carrier::Bool(value))
            }
            fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Carrier, E> {
                Ok(Carrier::Number(value as f64))
            }
            fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Carrier, E> {
                Ok(Carrier::Number(value as f64))
            }
            fn visit_i128<E: serde::de::Error>(self, value: i128) -> Result<Carrier, E> {
                Ok(Carrier::Number(value as f64))
            }
            fn visit_u128<E: serde::de::Error>(self, value: u128) -> Result<Carrier, E> {
                Ok(Carrier::Number(value as f64))
            }
            fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Carrier, E> {
                Ok(Carrier::Number(value))
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Carrier, E> {
                Ok(Carrier::String(value.to_owned()))
            }
            fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Carrier, E> {
                Ok(Carrier::String(value))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<Carrier, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = access.next_element()? {
                    values.push(value);
                }
                Ok(Carrier::Array(values))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Carrier, A::Error> {
                let mut values = Vec::new();
                while let Some((key, value)) = access.next_entry()? {
                    values.push((key, value));
                }
                Ok(Carrier::Object(values))
            }
        }
        deserializer.deserialize_any(ValueVisitor)
    }
}

fn syntax(message: &str) -> ToolError {
    ToolError(format!("SQLite write content must be valid JSON5: {message}"))
}
fn read_hex(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, count: usize) -> Result<u16, ToolError> {
    let mut value = 0u16;
    for _ in 0..count {
        let digit = chars.next().and_then(|ch| ch.to_digit(16)).ok_or_else(|| syntax("Invalid escape sequence"))?;
        value = (value << 4) | digit as u16;
    }
    Ok(value)
}

fn string_token(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, quote: char) -> Result<WireString, ToolError> {
    let mut units = Vec::new();
    while let Some(ch) = chars.next() {
        match ch {
            ch if ch == quote => return Ok(WireString::from_units(units)),
            '\r' | '\n' => return Err(syntax("Unterminated string")),
            '\\' => {
                let escaped = chars.next().ok_or_else(|| syntax("Unterminated escape sequence"))?;
                let unit = match escaped {
                    'u' => Some(read_hex(chars, 4)?),
                    'x' => Some(read_hex(chars, 2)?),
                    'b' => Some(8),
                    'f' => Some(12),
                    'n' => Some(10),
                    'r' => Some(13),
                    't' => Some(9),
                    'v' => Some(11),
                    '0' if !chars.peek().is_some_and(char::is_ascii_digit) => Some(0),
                    '0'..='9' => return Err(syntax("Invalid decimal escape sequence")),
                    '\n' | '\u{2028}' | '\u{2029}' => None,
                    '\r' => {
                        if chars.peek() == Some(&'\n') {
                            chars.next();
                        }
                        None
                    }
                    ch => {
                        let mut buffer = [0; 2];
                        units.extend_from_slice(ch.encode_utf16(&mut buffer));
                        None
                    }
                };
                if let Some(unit) = unit {
                    units.push(unit);
                }
            }
            ch => {
                let mut buffer = [0; 2];
                units.extend_from_slice(ch.encode_utf16(&mut buffer));
            }
        }
    }
    Err(syntax("Unterminated string"))
}

fn radix_number(digits: &str, width: u32) -> f64 {
    // Round the exact base-2/8/16 integer once, instead of once per digit.
    let digits = digits.trim_start_matches('0');
    if digits.is_empty() {
        return 0.0;
    }
    let radix = 1 << width;
    let first = (digits.as_bytes()[0] as char).to_digit(radix).unwrap();
    let bits = (32 - first.leading_zeros()) as usize + width as usize * (digits.len() - 1);
    let mut top = 0u64;
    let mut seen = 0usize;
    let mut round = false;
    let mut sticky = false;
    for (index, ch) in digits.chars().enumerate() {
        let nibble = ch.to_digit(radix).unwrap();
        let width = if index == 0 { 32 - nibble.leading_zeros() } else { width };
        for shift in (0..width).rev() {
            let bit = (nibble >> shift) & 1;
            if seen < 53 {
                top = (top << 1) | u64::from(bit);
            } else if seen == 53 {
                round = bit != 0;
            } else {
                sticky |= bit != 0;
            }
            seen += 1;
        }
    }
    if round && (sticky || top & 1 != 0) {
        top += 1;
    }
    let number = if bits <= 53 {
        top as f64
    } else if bits > 1100 {
        f64::INFINITY
    } else {
        (top as f64) * 2f64.powi((bits - 53) as i32)
    };
    number
}

fn hex_number(source: &str) -> f64 {
    let number = radix_number(&source.trim_start_matches(['+', '-'])[2..], 4);
    if source.starts_with('-') { -number } else { number }
}

fn real_key_number(key: &str) -> f64 {
    let trimmed = trim(key);
    let width = match trimmed.get(..2) {
        Some("0x" | "0X") => Some(4),
        Some("0o" | "0O") => Some(3),
        Some("0b" | "0B") => Some(1),
        _ => None,
    };
    if let Some(width) = width {
        let digits = &trimmed[2..];
        if digits.is_empty() || !digits.chars().all(|ch| ch.is_digit(1 << width)) {
            return f64::NAN;
        }
        return radix_number(digits, width);
    }
    ara_prompt::js::to_number(&serde_json::Value::String(key.to_owned()))
}

fn prepare_json5(source: &str) -> Result<(String, Vec<WireString>), ToolError> {
    static NUMBER: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?i)^[+-]?(?:0x[0-9a-f]+|(?:(?:0|[1-9][0-9]*)(?:\.[0-9]*)?|\.[0-9]+)(?:e[+-]?[0-9]+)?)$")
            .unwrap()
    });
    let mut chars = source.chars().peekable();
    let mut output = String::new();
    let mut strings = Vec::new();
    while let Some(ch) = chars.next() {
        match ch {
            '\'' | '"' => {
                let value = string_token(&mut chars, ch)?;
                output.push_str(&format!("\"\\u0000ARA{}\"", strings.len()));
                strings.push(value);
            }
            '/' if chars.peek() == Some(&'/') => {
                output.push(ch);
                output.push(chars.next().unwrap());
                for ch in chars.by_ref() {
                    output.push(ch);
                    if matches!(ch, '\r' | '\n' | '\u{2028}' | '\u{2029}') {
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                output.push(ch);
                output.push(chars.next().unwrap());
                let mut previous = '\0';
                for ch in chars.by_ref() {
                    output.push(ch);
                    if previous == '*' && ch == '/' {
                        break;
                    }
                    previous = ch;
                }
            }
            '+' | '-' | '.' | '0'..='9' => {
                let mut token = String::from(ch);
                while chars.peek().is_some_and(|ch| {
                    !is_space(*ch) && !matches!(ch, ',' | ':' | ']' | '}' | '/' | '[' | '{' | '\'' | '"')
                }) {
                    token.push(chars.next().unwrap());
                }
                if NUMBER.is_match(&token) {
                    if token.trim_start_matches(['+', '-']).starts_with("0x")
                        || token.trim_start_matches(['+', '-']).starts_with("0X")
                    {
                        let number = hex_number(&token);
                        output.push_str(&if number.is_infinite() {
                            if number.is_sign_negative() { "-Infinity".into() } else { "Infinity".into() }
                        } else {
                            format!("{number:?}")
                        });
                    } else if !token.contains(['.', 'e', 'E']) {
                        output.push_str(&token);
                        output.push_str(".0");
                    } else {
                        output.push_str(&token);
                    }
                } else {
                    output.push_str(&token);
                }
            }
            ch if !is_space(ch) && !matches!(ch, ',' | ':' | ']' | '}' | '/' | '[' | '{') => {
                output.push(ch);
                while chars.peek().is_some_and(|ch| {
                    !is_space(*ch) && !matches!(ch, ',' | ':' | ']' | '}' | '/' | '[' | '{' | '\'' | '"')
                }) {
                    output.push(chars.next().unwrap());
                }
            }
            ch => output.push(ch),
        }
    }
    Ok((output, strings))
}

fn restore_string(value: String, strings: &[WireString]) -> WireString {
    match value.strip_prefix("\0ARA").and_then(|index| index.parse::<usize>().ok()).and_then(|index| strings.get(index))
    {
        Some(value) => value.clone(),
        None => WireString::from(value),
    }
}

#[derive(Debug)]
enum Binding {
    Null,
    Integer(i64),
    Real(f64),
    Text(Vec<u8>),
}
impl ToSql for Binding {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Borrowed(match self {
            Self::Null => ValueRef::Null,
            Self::Integer(value) => ValueRef::Integer(*value),
            Self::Real(value) => ValueRef::Real(*value),
            Self::Text(bytes) => ValueRef::Text(bytes),
        }))
    }
}
fn number_binding(value: f64) -> Binding {
    if value.is_finite()
        && value.fract() == 0.0
        && value.abs() <= 9_007_199_254_740_991.0
        && !(value == 0.0 && value.is_sign_negative())
    {
        Binding::Integer(value as i64)
    } else {
        Binding::Real(value)
    }
}
fn array_index(key: &WireString) -> Option<u32> {
    let key = key.to_utf8().ok()?;
    let parsed = key.parse::<u32>().ok()?;
    (parsed != u32::MAX && key == parsed.to_string()).then_some(parsed)
}
fn object_entries(source: &str) -> Result<Vec<(WireString, Carrier)>, ToolError> {
    let (prepared, strings) = prepare_json5(source)?;
    let value: Carrier = json5::from_str(&prepared).map_err(|error| syntax(&error.to_string()))?;
    let Carrier::Object(entries) = value else {
        return Err(ToolError("SQLite write content must be a JSON object".into()));
    };
    let mut output: Vec<(WireString, Carrier)> = Vec::new();
    for (key, value) in entries {
        let key = restore_string(key, &strings);
        let value = match value {
            Carrier::String(value) => Carrier::Wire(restore_string(value, &strings)),
            value => value,
        };
        if let Some((_, existing)) = output.iter_mut().find(|(existing, _)| existing == &key) {
            *existing = value;
        } else {
            output.push((key, value));
        }
    }
    // JS Object.entries enumerates canonical array-index keys before strings.
    output.sort_by_key(|(key, _)| array_index(key).map_or((1, 0), |index| (0, index)));
    Ok(output)
}

fn scalar(value: Carrier, column: &str) -> Result<Binding, ToolError> {
    Ok(match value {
        Carrier::Null => Binding::Null,
        Carrier::Bool(value) => Binding::Integer(i64::from(value)),
        Carrier::Number(value) => number_binding(value),
        Carrier::Wire(value) => Binding::Text(value.sqlite_text_bytes()),
        Carrier::String(_) => unreachable!("quoted string restored before binding"),
        Carrier::Array(values) => {
            drop(values);
            return Err(ToolError(format!("SQLite column '{column}' only accepts JSON scalar values or null")));
        }
        Carrier::Object(_) => {
            return Err(ToolError(format!("SQLite column '{column}' only accepts JSON scalar values or null")));
        }
    })
}
fn quoted(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn schema(db: &Connection, table: &str) -> Result<(String, Vec<(String, String, i64)>), ToolError> {
    let sql: Option<Option<String>> = db
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name = ?",
            [table],
            |row| row.get(0),
        )
        .optional()
        .map_err(tool_error)?;
    let Some(sql) = sql else {
        return Err(ToolError(format!("SQLite table '{table}' not found")));
    };
    let mut query = db.prepare(&format!("PRAGMA table_info({})", quoted(table))).map_err(tool_error)?;
    let columns = query
        .query_map([], |row| Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, i64>(5)?)))
        .map_err(tool_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(tool_error)?;
    Ok((sql.unwrap_or_default(), columns))
}
fn integer_key(key: &str, label: &str) -> Result<Binding, ToolError> {
    let trimmed = trim(key);
    let digits = trimmed.strip_prefix('-').unwrap_or(trimmed);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ToolError(format!("{label} must be an integer; got '{key}'")));
    }
    if let Ok(number) = trimmed.parse::<f64>() {
        if number.abs() <= 9_007_199_254_740_991.0 {
            return Ok(number_binding(number));
        }
    }
    // Bun binds BigInt through asInt64, including modulo wrap at 64 bits.
    let number = digits.bytes().fold(0u64, |value, digit| value.wrapping_mul(10).wrapping_add(u64::from(digit - b'0')));
    Ok(Binding::Integer(if trimmed.starts_with('-') { number.wrapping_neg() as i64 } else { number as i64 }))
}
fn lookup_key(key: &str, kind: &str) -> Result<Binding, ToolError> {
    let kind = trim(kind).to_uppercase();
    if kind.contains("INT") {
        return integer_key(key, &format!("Primary key '{key}'"));
    }
    if kind.contains("REAL") || kind.contains("FLOA") || kind.contains("DOUB") {
        // Radix integers round once as Number(key); preserve the existing
        // decimal, nonfinite and invalid-spelling behavior for other inputs.
        let number = real_key_number(key);
        if number.is_finite() {
            return Ok(number_binding(number));
        }
    }
    Ok(Binding::Text(key.as_bytes().to_vec()))
}

pub(crate) fn execute(display: &str, content: &str, target: &Target) -> Result<String, ToolError> {
    if !target.exists {
        return Err(ToolError(format!("SQLite database '{display}' not found")));
    }
    let db = Connection::open_with_flags(&target.absolute, OpenFlags::SQLITE_OPEN_READ_WRITE).map_err(tool_error)?;
    db.busy_timeout(Duration::from_millis(3000)).map_err(tool_error)?;
    let empty = trim(content).is_empty();
    if empty && target.key.is_none() {
        return Err(ToolError("SQLite deletes require a row key in the path".into()));
    }
    // Parsing precedes schema lookup in the original nonempty write branch.
    let entries = if empty { Vec::new() } else { object_entries(content)? };
    let (sql, columns) = schema(&db, &target.table)?;
    let key_clause = if target.key.is_some() {
        let mut primary: Vec<_> = columns.iter().filter(|(_, _, rank)| *rank > 0).collect();
        primary.sort_by_key(|(_, _, rank)| *rank);
        match primary.as_slice() {
            [column] => Some((quoted(&column.0), Some(column.1.as_str()))),
            [] => {
                static WITHOUT: LazyLock<regex::Regex> =
                    LazyLock::new(|| regex::Regex::new(r"(?i)\bWITHOUT\s+ROWID\b").unwrap());
                if WITHOUT.is_match(&sql) {
                    return Err(ToolError(format!(
                        "SQLite table '{}' does not expose ROWID; use '?where=' instead",
                        target.table
                    )));
                }
                Some(("rowid".into(), None))
            }
            _ => {
                return Err(ToolError(format!(
                    "SQLite table '{}' has a composite primary key; use '?where=' instead",
                    target.table
                )));
            }
        }
    } else {
        None
    };
    let table = quoted(&target.table);
    if empty {
        let Some((column, kind)) = key_clause else {
            return Err(ToolError("SQLite deletes require a row key in the path".into()));
        };
        let key = target.key.as_ref().unwrap();
        let binding = match kind {
            Some(kind) => lookup_key(key, kind)?,
            None => integer_key(key, "SQLite ROWID")?,
        };
        let deleted = db
            .execute(&format!("DELETE FROM {table} WHERE {column} = ?"), params_from_iter([&binding]))
            .map_err(tool_error)?;
        return Ok(if deleted > 0 {
            format!("Deleted row '{}' from {}", target.key.as_ref().unwrap(), target.table)
        } else {
            format!("No row deleted from {} for key '{}'", target.table, target.key.as_ref().unwrap())
        });
    }
    let mut bindings = Vec::new();
    let mut names = Vec::new();
    for (name, value) in entries {
        let column = columns.iter().find(|(column, _, _)| name == WireString::from(column.as_str()));
        let Some((column, _, _)) = column else {
            return Err(ToolError(format!(
                "SQLite table '{}' has no column named '{}'",
                target.table,
                String::from_utf16_lossy(name.units())
            )));
        };
        names.push(quoted(column));
        bindings.push(scalar(value, column)?);
    }
    if let Some((column, kind)) = key_clause {
        if bindings.is_empty() {
            return Err(ToolError("SQLite updates require at least one column value".into()));
        }
        let key = target.key.as_ref().unwrap();
        let key = match kind {
            Some(kind) => lookup_key(key, kind)?,
            None => integer_key(key, "SQLite ROWID")?,
        };
        let assignments = names.iter().map(|column| format!("{column} = ?")).collect::<Vec<_>>().join(", ");
        bindings.push(key);
        let updated = db
            .execute(&format!("UPDATE {table} SET {assignments} WHERE {column} = ?"), params_from_iter(bindings.iter()))
            .map_err(tool_error)?;
        Ok(if updated > 0 {
            format!("Updated row '{}' in {}", target.key.as_ref().unwrap(), target.table)
        } else {
            format!("No row updated in {} for key '{}'", target.table, target.key.as_ref().unwrap())
        })
    } else {
        if bindings.is_empty() {
            db.execute(&format!("INSERT INTO {table} DEFAULT VALUES"), []).map_err(tool_error)?;
        } else {
            let placeholders = vec!["?"; bindings.len()].join(", ");
            db.execute(
                &format!("INSERT INTO {table} ({}) VALUES ({placeholders})", names.join(", ")),
                params_from_iter(bindings.iter()),
            )
            .map_err(tool_error)?;
        }
        Ok(format!("Inserted row into {}", target.table))
    }
}
