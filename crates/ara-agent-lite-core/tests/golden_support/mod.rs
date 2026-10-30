//! Golden files of the policy-off paths (requirements 17.7, 18.8, 19.7) under
//! `tests/golden/<name>/`. Tests compare by default; `ARA_UPDATE_GOLDEN=1`
//! rewrites the files instead. Volatile values are normalized first; each
//! golden directory's README.md lists the rules.
#![allow(dead_code)]

use std::{collections::HashMap, path::Path};

use ara_agent_lite_core::session_journal::Journal;
use serde_json::{Map, Value};

/// Numeric fields whose values are wall-clock or elapsed times.
const VOLATILE_NUMBERS: &[&str] =
    &["timestamp", "timestamp_ms", "milliseconds_remaining", "duration_ms", "elapsed_ms", "deadline_unix_ms"];

pub fn dir(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden").join(name)
}

/// Replaces the temp root and endpoint with placeholders, zeroes the
/// [`VOLATILE_NUMBERS`] (also inside JSON text embedded in strings), and
/// numbers random identifiers: 24-hex runs (`fresh_id`) become `%ID-n%`,
/// 64-hex runs (SHA-256) `%SHA-n%`, by first appearance.
pub struct Normalizer {
    literals: Vec<(String, &'static str)>,
    ids: HashMap<String, String>,
    shas: HashMap<String, String>,
}

impl Normalizer {
    pub fn new(root: &Path, endpoint: &str) -> Self {
        let display = root.display().to_string();
        let plain = display.trim_start_matches(r"\\?\").to_owned();
        let mut literals = Vec::new();
        for form in [display.clone(), plain.clone()] {
            literals.push((form.replace('\\', "\\\\"), "%ROOT%"));
            literals.push((form.replace('\\', "/"), "%ROOT%"));
            literals.push((form, "%ROOT%"));
        }
        literals.push((endpoint.trim_end_matches("/v1").to_owned(), "%ENDPOINT%"));
        // Longest first, so a prefix form never splits a longer one.
        literals.sort_by_key(|(form, _)| std::cmp::Reverse(form.len()));
        literals.dedup();
        Self { literals, ids: HashMap::new(), shas: HashMap::new() }
    }

    pub fn value(&mut self, value: &Value) -> Value {
        match value {
            Value::String(text) => Value::String(self.text(text)),
            Value::Array(items) => Value::Array(items.iter().map(|item| self.value(item)).collect()),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(key, item)| {
                        let item = match item {
                            Value::Number(_) if VOLATILE_NUMBERS.contains(&key.as_str()) => Value::from(0),
                            other => self.value(other),
                        };
                        (key.clone(), item)
                    })
                    .collect::<Map<_, _>>(),
            ),
            other => other.clone(),
        }
    }

    fn text(&mut self, text: &str) -> String {
        let mut text = self.literals.iter().fold(text.to_owned(), |text, (form, label)| text.replace(form, label));
        for key in VOLATILE_NUMBERS {
            for quote in ["\"", "\\\""] {
                let marker = format!("{quote}{key}{quote}:");
                let mut from = 0;
                while let Some(found) = text[from..].find(&marker) {
                    let start = from + found + marker.len();
                    let end = start + text[start..].bytes().take_while(u8::is_ascii_digit).count();
                    text.replace_range(start..end, "0");
                    from = start + 1;
                }
            }
        }
        // Long single-character fillers (scenario padding) are kept as counts.
        let mut compact = String::with_capacity(text.len());
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            let mut count = 1;
            while chars.peek() == Some(&c) {
                chars.next();
                count += 1;
            }
            if count >= 64 {
                compact.push_str(&format!("%REPEAT({c},{count})%"));
            } else {
                compact.extend(std::iter::repeat_n(c, count));
            }
        }
        let text = compact;
        let bytes = text.as_bytes();
        let mut out = String::with_capacity(text.len());
        let mut index = 0;
        while index < bytes.len() {
            let hex = bytes[index..].iter().take_while(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b)).count();
            if hex == 0 {
                let next = index + text[index..].chars().next().map_or(1, char::len_utf8);
                out.push_str(&text[index..next]);
                index = next;
                continue;
            }
            let run = &text[index..index + hex];
            let (table, prefix) = match hex {
                24 => (&mut self.ids, "ID"),
                64 => (&mut self.shas, "SHA"),
                _ => {
                    out.push_str(run);
                    index += hex;
                    continue;
                }
            };
            let next = table.len() + 1;
            out.push_str(table.entry(run.to_owned()).or_insert_with(|| format!("%{prefix}-{next}%")));
            index += hex;
        }
        out
    }
}

/// Event order with each `model_result` moved before the `text_delta` run it
/// raced with (the canonical order of `event_compat`, DR-P4-1).
pub fn canonical(events: &[Value]) -> Vec<Value> {
    let mut events = events.to_vec();
    for index in 0..events.len() {
        if events[index]["type"] == "model_result" {
            let mut at = index;
            while at > 0 && events[at - 1]["type"] == "text_delta" {
                events.swap(at - 1, at);
                at -= 1;
            }
        }
    }
    events
}

/// Event `type` sequence in canonical order.
pub fn event_types(events: &[Value]) -> Value {
    Value::Array(canonical(events).iter().map(|event| event["type"].clone()).collect())
}

/// Every raw journal record of the Session at `session`.
pub fn journal_entries(session: &Path) -> Value {
    let journal = Journal::for_session(session);
    let count = journal.open().expect("Session journal").raw_count;
    serde_json::to_value(journal.read(0, count, 64 * 1024 * 1024).expect("journal records")).unwrap()
}

/// Compare `actual` with `tests/golden/<name>/<file>`, or write it when
/// `ARA_UPDATE_GOLDEN=1`.
pub fn check(name: &str, file: &str, actual: &Value) {
    let path = dir(name).join(file);
    let mut text = serde_json::to_string_pretty(actual).unwrap();
    text.push('\n');
    if std::env::var("ARA_UPDATE_GOLDEN").as_deref() == Ok("1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        return;
    }
    let expected = std::fs::read(&path)
        .unwrap_or_else(|error| panic!("{}: {error}; run with ARA_UPDATE_GOLDEN=1 to capture it", path.display()));
    let expected: Value = serde_json::from_slice(&expected).unwrap();
    assert!(&expected == actual, "{} differs from the golden; actual:\n{text}", path.display());
}
