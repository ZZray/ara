//! Small helpers shared by the store, HTTP layer and CLI.

use crate::error::{Error, Result};
use std::time::Duration;

/// `n` random bytes rendered as lowercase hex (Go: `randomID`).
pub fn random_hex(n: usize) -> Result<String> {
    let mut buf = vec![0u8; n];
    getrandom::fill(&mut buf).map_err(|e| Error::new(format!("random source failed: {e}")))?;
    let mut out = String::with_capacity(n * 2);
    for b in buf {
        out.push_str(&format!("{b:02x}"));
    }
    Ok(out)
}

/// Constant-time token comparison; empty tokens never match (Go: `tokenEqual`).
pub fn token_equal(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Constant-time byte comparison (Go: `equalBytes`).
pub fn equal_bytes(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Parse a Go `time.ParseDuration` string ("25s", "1m30s", "500ms", "0", "-1.5h")
/// into signed nanoseconds.
pub fn parse_go_duration(input: &str) -> Result<i128> {
    let invalid = || Error::new(format!("invalid duration {input:?}"));
    let mut rest = input;
    let mut negative = false;
    if let Some(stripped) = rest.strip_prefix('-') {
        negative = true;
        rest = stripped;
    } else if let Some(stripped) = rest.strip_prefix('+') {
        rest = stripped;
    }
    if rest == "0" {
        return Ok(0);
    }
    if rest.is_empty() {
        return Err(invalid());
    }
    let mut total: f64 = 0.0;
    while !rest.is_empty() {
        let digits_end = rest.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(rest.len());
        if digits_end == 0 {
            return Err(invalid());
        }
        let number: f64 = rest[..digits_end].parse().map_err(|_| invalid())?;
        rest = &rest[digits_end..];
        let unit_end = rest.find(|c: char| c.is_ascii_digit() || c == '.').unwrap_or(rest.len());
        if unit_end == 0 {
            return Err(Error::new(format!("missing unit in duration {input:?}")));
        }
        let scale: f64 = match &rest[..unit_end] {
            "ns" => 1.0,
            "us" | "\u{b5}s" | "\u{3bc}s" => 1e3,
            "ms" => 1e6,
            "s" => 1e9,
            "m" => 60.0 * 1e9,
            "h" => 3600.0 * 1e9,
            other => return Err(Error::new(format!("unknown unit {other:?} in duration {input:?}"))),
        };
        rest = &rest[unit_end..];
        total += number * scale;
    }
    let nanos = total.round() as i128;
    Ok(if negative { -nanos } else { nanos })
}

/// Duration string accepted by both this server and Go's `time.ParseDuration`.
pub fn format_wait(wait: Duration) -> String {
    format!("{}ms", wait.as_millis())
}

/// True when the string contains a non-printable character (control or unassigned).
pub fn has_nonprintable(s: &str) -> bool {
    s.chars().any(|c| c.is_control())
}

/// Convert signed nanoseconds into a `Duration`; negative values are rejected.
pub fn duration_from_nanos(nanos: i128) -> Option<Duration> {
    if nanos < 0 {
        return None;
    }
    let secs = u64::try_from(nanos / 1_000_000_000).ok()?;
    Some(Duration::new(secs, (nanos % 1_000_000_000) as u32))
}

/// Re-indent compact JSON text with two spaces (Go: `json.Indent`). Key order and
/// number spelling are preserved because the text is never parsed into a value.
pub fn indent_json(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() * 2);
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    // Set right after `{` or `[`: the newline is only emitted if the container is not empty.
    let mut pending_open = false;
    let newline = |out: &mut String, depth: usize| {
        out.push('\n');
        for _ in 0..depth {
            out.push_str("  ");
        }
    };
    for c in raw.chars() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        if c.is_whitespace() {
            continue;
        }
        if pending_open {
            pending_open = false;
            if c == '}' || c == ']' {
                out.push(c);
                depth -= 1;
                continue;
            }
            newline(&mut out, depth);
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '{' | '[' => {
                out.push(c);
                depth += 1;
                pending_open = true;
            }
            '}' | ']' => {
                depth = depth.saturating_sub(1);
                newline(&mut out, depth);
                out.push(c);
            }
            ',' => {
                out.push(c);
                newline(&mut out, depth);
            }
            ':' => out.push_str(": "),
            _ => out.push(c),
        }
    }
    out
}

/// Decode `%XX` escapes and `+` in a URL query component.
pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() && bytes[i + 1].is_ascii_hexdigit() && bytes[i + 2].is_ascii_hexdigit() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("00");
                out.push(u8::from_str_radix(hex, 16).unwrap_or(0));
                i += 2;
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// First value of `key` in a raw query string (`a=1&b=2`).
pub fn query_value(query: &str, key: &str) -> Option<String> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| percent_decode(k) == key)
        .map(|(_, v)| percent_decode(v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_duration_forms() {
        assert_eq!(parse_go_duration("25s").unwrap(), 25_000_000_000);
        assert_eq!(parse_go_duration("1m30s").unwrap(), 90_000_000_000);
        assert_eq!(parse_go_duration("500ms").unwrap(), 500_000_000);
        assert_eq!(parse_go_duration("0").unwrap(), 0);
        assert_eq!(parse_go_duration("-1s").unwrap(), -1_000_000_000);
        assert_eq!(parse_go_duration("1.5s").unwrap(), 1_500_000_000);
        assert!(parse_go_duration("").is_err());
        assert!(parse_go_duration("5").is_err());
        assert!(parse_go_duration("abc").is_err());
        assert!(parse_go_duration("5x").is_err());
    }

    #[test]
    fn token_compare_rejects_empty_and_mismatch() {
        assert!(token_equal("abc", "abc"));
        assert!(!token_equal("", ""));
        assert!(!token_equal("abc", "abd"));
        assert!(!token_equal("abc", "abcd"));
    }

    #[test]
    fn indent_matches_go_json_indent() {
        assert_eq!(
            indent_json(r#"{"a":[1,2],"b":{},"c":[],"d":"x,y:{"}"#),
            "{\n  \"a\": [\n    1,\n    2\n  ],\n  \"b\": {},\n  \"c\": [],\n  \"d\": \"x,y:{\"\n}"
        );
        assert_eq!(indent_json("9007199254740993"), "9007199254740993");
    }

    #[test]
    fn query_values_decode() {
        assert_eq!(query_value("timeout=25s&x=1", "timeout").as_deref(), Some("25s"));
        assert_eq!(query_value("timeout=1%2E5s", "timeout").as_deref(), Some("1.5s"));
        assert_eq!(query_value("x=1", "timeout"), None);
    }

    #[test]
    fn random_hex_length() {
        assert_eq!(random_hex(12).unwrap().len(), 24);
        assert_ne!(random_hex(16).unwrap(), random_hex(16).unwrap());
    }
}
