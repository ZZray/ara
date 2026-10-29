//! Bounded, sanitized and redacted text for errors, status and tool output.

/// Claude Code `recursivelySanitizeUnicode` without NFC: drops C0 controls
/// other than tab/newline/carriage return, DEL and U+FFFD.
pub fn sanitize(text: &str) -> String {
    text.chars()
        .filter(|&c| !matches!(c, '\u{0}'..='\u{8}' | '\u{b}' | '\u{c}' | '\u{e}'..='\u{1f}' | '\u{7f}' | '\u{fffd}'))
        .collect()
}

/// First `max` characters, marked when cut.
pub(crate) fn bounded(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((cut, _)) => format!("{}... [truncated]", &text[..cut]),
        None => text.to_string(),
    }
}

/// Last `max` characters.
pub(crate) fn tail(text: &str, max: usize) -> &str {
    let count = text.chars().count();
    if count <= max {
        return text;
    }
    let (cut, _) = text.char_indices().nth(count - max).expect("index within text");
    &text[cut..]
}

/// Configuration values (expanded `env`/`headers` values and every expanded
/// variable value) that must never appear in errors, status or tool output.
#[derive(Clone, Default)]
pub(crate) struct Redactor(Vec<String>);

impl Redactor {
    /// Values shorter than this are not redacted: they would erase ordinary
    /// words and are not plausible credentials.
    const MIN: usize = 4;

    pub(crate) fn new(mut secrets: Vec<String>) -> Self {
        secrets.retain(|secret| secret.chars().count() >= Self::MIN);
        secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
        secrets.dedup();
        Self(secrets)
    }

    pub(crate) fn apply(&self, text: &str) -> String {
        let mut text = text.to_string();
        for secret in &self.0 {
            if text.contains(secret.as_str()) {
                text = text.replace(secret.as_str(), "[redacted]");
            }
        }
        text
    }

    /// Server-originated text prepared for an error or status line.
    pub(crate) fn server_text(&self, text: &str, max: usize) -> String {
        bounded(&self.apply(&sanitize(text)), max)
    }
}
