//! Stateful ECMAScript regular expressions for the fixed OMP catalog seams.
//!
//! `regress` supplies native matching. This wrapper owns the JavaScript flags,
//! UTF-16 offsets and `lastIndex` behavior that a matching engine does not own.
//! A template must share the same instance when it shares a RegExp upstream.

use std::{fmt, ops::Range};

use ara_rpc::WireString;
use regress::{Flags, Regex};

const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsRegexError {
    pub name: &'static str,
    pub message: String,
    /// Retain the native diagnostic when translating the runtime's wording.
    pub native_message: Option<String>,
}

impl fmt::Display for JsRegexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for JsRegexError {}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct JsFlags {
    indices: bool,
    global: bool,
    ignore_case: bool,
    multiline: bool,
    dot_all: bool,
    unicode: bool,
    unicode_sets: bool,
    sticky: bool,
}

impl JsFlags {
    fn parse(input: &str) -> Result<Self, JsRegexError> {
        let mut result = Self::default();
        for flag in input.chars() {
            let slot = match flag {
                'd' => &mut result.indices,
                'g' => &mut result.global,
                'i' => &mut result.ignore_case,
                'm' => &mut result.multiline,
                's' => &mut result.dot_all,
                'u' => &mut result.unicode,
                'v' => &mut result.unicode_sets,
                'y' => &mut result.sticky,
                _ => return Err(invalid_flags()),
            };
            if *slot {
                return Err(invalid_flags());
            }
            *slot = true;
        }
        if result.unicode && result.unicode_sets {
            return Err(invalid_flags());
        }
        Ok(result)
    }

    fn canonical(self) -> String {
        [
            ('d', self.indices),
            ('g', self.global),
            ('i', self.ignore_case),
            ('m', self.multiline),
            ('s', self.dot_all),
            ('u', self.unicode),
            ('v', self.unicode_sets),
            ('y', self.sticky),
        ]
        .into_iter()
        .filter_map(|(flag, enabled)| enabled.then_some(flag))
        .collect()
    }

    fn full_unicode(self) -> bool {
        self.unicode || self.unicode_sets
    }

    fn stateful(self) -> bool {
        self.global || self.sticky
    }

    fn engine(self) -> Flags {
        Flags {
            icase: self.ignore_case,
            multiline: self.multiline,
            dot_all: self.dot_all,
            // UnicodeSets has the Unicode parser, folding and decoding rules.
            // Keep the original u/v distinction in the public flag metadata.
            unicode: self.full_unicode(),
            unicode_sets: self.unicode_sets,
            ..Flags::default()
        }
    }
}

fn invalid_flags() -> JsRegexError {
    JsRegexError {
        name: "SyntaxError",
        message: "Invalid flags supplied to RegExp constructor.".into(),
        native_message: None,
    }
}

/// A single `RegExp.prototype.exec` result. Captures include group zero;
/// `None` means the group did not participate, while an empty string matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsRegexMatch {
    pub index: usize,
    pub end: usize,
    pub captures: Vec<Option<WireString>>,
    /// Present only for expressions with named groups; declaration order wins.
    pub groups: Option<Vec<(WireString, Option<WireString>)>>,
    /// Present only with `d`; offsets are UTF-16 code units, including group 0.
    pub indices: Option<Vec<Option<Range<usize>>>>,
    pub group_indices: Option<Vec<(WireString, Option<Range<usize>>)>>,
}

/// Native state of one JavaScript RegExp. It is deliberately not `Clone`:
/// consumers share a handle when the upstream object identity is shared.
pub struct JsRegExp {
    source: WireString,
    flags: JsFlags,
    canonical_flags: String,
    last_index: f64,
    regex: Regex,
}

impl fmt::Debug for JsRegExp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JsRegExp")
            .field("source", &self.source)
            .field("flags", &self.canonical_flags)
            .field("last_index", &self.last_index)
            .finish_non_exhaustive()
    }
}

impl JsRegExp {
    pub fn new(pattern: WireString, flags: &str) -> Result<Self, JsRegexError> {
        let flags = JsFlags::parse(flags)?;
        // Parsing a non-u expression must see two atoms for a surrogate pair.
        // Unicode parsing combines pairs, while retaining lone surrogates.
        let points: Vec<u32> = if flags.full_unicode() {
            std::char::decode_utf16(pattern.units().iter().copied())
                .map(|unit| unit.map_or_else(|error| u32::from(error.unpaired_surrogate()), u32::from))
                .collect()
        } else {
            pattern.units().iter().copied().map(u32::from).collect()
        };
        let regex = Regex::from_unicode(points.into_iter(), flags.engine())
            .map_err(|error| syntax_error(error.to_string(), &pattern))?;
        Ok(Self {
            source: escape_source(&pattern, flags.unicode_sets),
            flags,
            canonical_flags: flags.canonical(),
            last_index: 0.0,
            regex,
        })
    }

    pub fn source(&self) -> &WireString {
        &self.source
    }

    pub fn flags(&self) -> &str {
        &self.canonical_flags
    }

    pub fn last_index(&self) -> f64 {
        self.last_index
    }

    /// JavaScript stores the assigned number; ToLength occurs only on exec.
    pub fn set_last_index(&mut self, value: f64) {
        self.last_index = value;
    }

    pub fn exec(&mut self, input: &WireString) -> Option<JsRegexMatch> {
        let units = input.units();
        let stateful = self.flags.stateful();
        let mut start = if stateful { to_length(self.last_index) } else { 0 };
        if start > units.len() {
            if stateful {
                self.last_index = 0.0;
            }
            return None;
        }
        // Bun/ECMAScript Unicode exec accepts a lastIndex in the middle of a
        // pair and starts at the pair's high surrogate, including sticky mode.
        if self.flags.full_unicode()
            && start > 0
            && start < units.len()
            && is_low_surrogate(units[start])
            && is_high_surrogate(units[start - 1])
        {
            start -= 1;
        }
        let matched = if self.flags.full_unicode() {
            self.regex.find_from_utf16(units, start).next()
        } else {
            self.regex.find_from_ucs2(units, start).next()
        };
        let Some(matched) = matched.filter(|value| !self.flags.sticky || value.start() == start) else {
            if stateful {
                self.last_index = 0.0;
            }
            return None;
        };
        if stateful {
            // RegExp#exec does not advance an empty match. Iteration helpers
            // own their own progress rule and must not change this object.
            self.last_index = matched.end() as f64;
        }
        let ranges: Vec<_> = matched.groups().collect();
        let captures = ranges.iter().map(|range| capture(units, range.as_ref())).collect();
        let mut names: Vec<(WireString, Option<Range<usize>>)> = Vec::new();
        for (name, range) in matched.named_groups() {
            let name = WireString::from(name);
            if let Some((_, previous)) = names.iter_mut().find(|(known, _)| *known == name) {
                // Mutually exclusive alternatives may declare the same name.
                if range.is_some() {
                    *previous = range;
                }
            } else {
                names.push((name, range));
            }
        }
        let groups = (!names.is_empty())
            .then(|| names.iter().map(|(name, range)| (name.clone(), capture(units, range.as_ref()))).collect());
        let group_indices = (self.flags.indices && !names.is_empty()).then_some(names);
        Some(JsRegexMatch {
            index: matched.start(),
            end: matched.end(),
            captures,
            groups,
            indices: self.flags.indices.then_some(ranges),
            group_indices,
        })
    }
}

fn capture(input: &[u16], range: Option<&Range<usize>>) -> Option<WireString> {
    range.map(|range| WireString::from_units(input[range.clone()].to_vec()))
}

fn to_length(value: f64) -> usize {
    if value.is_nan() || value <= 0.0 { 0 } else { value.floor().min(MAX_SAFE_INTEGER) as usize }
}

fn is_high_surrogate(unit: u16) -> bool {
    (0xd800..=0xdbff).contains(&unit)
}

fn is_low_surrogate(unit: u16) -> bool {
    (0xdc00..=0xdfff).contains(&unit)
}

fn append_line_escape(output: &mut Vec<u16>, unit: u16) -> bool {
    let escaped = match unit {
        0x0a => "\\n",
        0x0d => "\\r",
        0x2028 => "\\u2028",
        0x2029 => "\\u2029",
        _ => return false,
    };
    output.extend(escaped.encode_utf16());
    true
}

/// `RegExp.prototype.source` escapes literal delimiters and line terminators;
/// existing escapes remain intact, including raw surrogate code units.
fn escape_source(pattern: &WireString, unicode_sets: bool) -> WireString {
    if pattern.is_empty() {
        return WireString::from("(?:)");
    }
    let input = pattern.units();
    let mut output = Vec::with_capacity(input.len());
    let mut class_depth = 0_usize;
    let mut index = 0;
    while index < input.len() {
        let unit = input[index];
        if unit == u16::from(b'\\') && index + 1 < input.len() {
            let next = input[index + 1];
            if !append_line_escape(&mut output, next) {
                output.push(unit);
                output.push(next);
            }
            index += 2;
            continue;
        }
        if unit == u16::from(b'[') && (class_depth == 0 || unicode_sets) {
            class_depth += 1;
        } else if unit == u16::from(b']') {
            class_depth = class_depth.saturating_sub(1);
        } else if unit == u16::from(b'/') && class_depth == 0 {
            output.push(u16::from(b'\\'));
        }
        if !append_line_escape(&mut output, unit) {
            output.push(unit);
        }
        index += 1;
    }
    WireString::from_units(output)
}

fn syntax_error(native: String, pattern: &WireString) -> JsRegexError {
    // These messages are the fixed Bun 1.4.0 constructor's observable wording.
    // Keep every unmatched engine diagnostic visible instead of treating a
    // rejected expression as an empty matcher or an unknown alias.
    let detail = match native.as_str() {
        "Unbalanced bracket" | "Unbalanced class set bracket" => "missing terminating ] for character class",
        "Unbalanced parenthesis" if has_unclosed_group(pattern) => "missing )",
        "Unbalanced parenthesis" => "unmatched parentheses",
        "Incomplete escape" | "Unterminated escape" => "\\ at end of pattern",
        "Invalid atom character" | "Quantifier not allowed here" => "nothing to repeat",
        "Invalid quantifier" => "numbers out of order in {} quantifier",
        "Invalid character range" | "Range values reversed, start char code is greater than end char code." => {
            "range out of order in character class"
        }
        "Invalid character escape" => "invalid escaped character for Unicode pattern",
        "Invalid class set character" => "invalid class set character",
        "Invalid class set operation"
        | "Unexpected character in class set intersection"
        | "Unexpected character in class set subtraction" => "invalid operation in class set",
        "Negated class set may contain strings" => "negated class set may contain strings",
        "Duplicate capture group name" => "duplicate group specifier name",
        "Invalid group modifier" => "unrecognized character after (?",
        "Invalid property name" | "Invalid character at property escape start" | "Invalid property escape" => {
            "invalid property expression"
        }
        _ => &native,
    };
    JsRegexError {
        name: "SyntaxError",
        message: format!("Invalid regular expression: {detail}"),
        native_message: Some(native),
    }
}

fn has_unclosed_group(pattern: &WireString) -> bool {
    let mut escaped = false;
    let mut class = false;
    let mut depth = 0_usize;
    for &unit in pattern.units() {
        if escaped {
            escaped = false;
        } else if unit == u16::from(b'\\') {
            escaped = true;
        } else if unit == u16::from(b'[') {
            class = true;
        } else if unit == u16::from(b']') {
            class = false;
        } else if !class && unit == u16::from(b'(') {
            depth += 1;
        } else if !class && unit == u16::from(b')') {
            depth = depth.saturating_sub(1);
        }
    }
    depth > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn re(source: &str, flags: &str) -> JsRegExp {
        JsRegExp::new(source.into(), flags).unwrap()
    }

    #[test]
    fn flags_are_validated_and_canonicalized() {
        assert_eq!(re("a", "ysmigdu").flags(), "dgimsuy");
        assert_eq!(re("a", "vg").flags(), "gv");
        for flags in ["z", "ii", "uv", "uvv", "G", "é"] {
            let error = JsRegExp::new("a".into(), flags).unwrap_err();
            assert_eq!(error, invalid_flags(), "{flags}");
        }
    }

    #[test]
    fn source_preserves_the_literal_and_escapes_only_delimiters_and_line_breaks() {
        for (source, expected) in [
            ("", "(?:)"),
            ("/", "\\/"),
            ("[/]", "[/]"),
            ("\\/", "\\/"),
            ("\\\\/", "\\\\\\/"),
            ("\n", "\\n"),
            ("\\\n", "\\n"),
            ("[\n]", "[\\n]"),
            ("\r\u{2028}\u{2029}", "\\r\\u2028\\u2029"),
        ] {
            assert_eq!(re(source, "").source(), &WireString::from(expected), "{source:?}");
        }
    }

    #[test]
    fn unicode_and_legacy_matching_keep_their_different_code_unit_offsets() {
        let input = WireString::from("😀");
        for flags in ["g", "y"] {
            let mut regex = re("(.)", flags);
            regex.set_last_index(1.0);
            let matched = regex.exec(&input).unwrap();
            assert_eq!((matched.index, matched.end), (1, 2));
            assert_eq!(matched.captures[1].as_ref().unwrap().units(), &[0xde00]);
            assert_eq!(regex.last_index(), 2.0);
        }
        for flags in ["gu", "yu", "gv", "yv"] {
            let mut regex = re("(.)", flags);
            regex.set_last_index(1.0);
            let matched = regex.exec(&input).unwrap();
            assert_eq!((matched.index, matched.end), (0, 2));
            assert_eq!(matched.captures[1], Some(input.clone()));
            assert_eq!(regex.last_index(), 2.0);
        }
    }

    #[test]
    fn lone_surrogates_are_valid_input_and_pattern_atoms() {
        for flags in ["", "u", "v"] {
            let source = WireString::from_units(vec![0xdc00]);
            let mut regex = JsRegExp::new(source.clone(), flags).unwrap();
            assert_eq!(regex.source(), &source);
            assert_eq!(regex.exec(&source).unwrap().captures, vec![Some(source)]);
        }
    }

    #[test]
    fn legacy_i_does_not_fold_non_ascii_characters_into_ascii() {
        for input in ["ſ", "K"] {
            assert!(re("[a-z]", "i").exec(&input.into()).is_none());
            assert!(re("[a-z]", "iu").exec(&input.into()).is_some());
            assert!(re("[a-z]", "iv").exec(&input.into()).is_some());
        }
        assert!(re("[^a-z]", "i").exec(&"ſ".into()).is_some());
        assert!(re("\\w", "i").exec(&"ſ".into()).is_none());
        assert!(re("\\W", "i").exec(&"ſ".into()).is_some());
        assert!(re("(?i:[a-z])", "").exec(&"ſ".into()).is_none());
        assert!(re("(?-i:[a-z])", "i").exec(&"A".into()).is_none());
        assert!(re("[σ]", "i").exec(&"ς".into()).is_some());
    }

    #[test]
    fn legacy_canonicalize_applies_to_literals_classes_and_backreferences() {
        for (pattern, input) in [
            ("s", "ſ"),
            ("i", "ı"),
            ("(s)\\1", "sſ"),
            ("(.)\\1", "ſS"),
            ("(?i:(s)\\1)", "sſ"),
            ("ᾀ", "ᾈ"),
            ("[ᾀ]", "ᾈ"),
            ("(.)\\1", "ᾀᾈ"),
        ] {
            assert!(re(pattern, "i").exec(&input.into()).is_none(), "{pattern} / {input}");
        }
        assert!(re("(s)\\1", "i").exec(&"sS".into()).is_some());
        assert!(re("[ᾀ]", "iu").exec(&"ᾈ".into()).is_some());
        assert!(re("[ᾀ]", "iv").exec(&"ᾈ".into()).is_some());
    }

    #[test]
    fn unicode_sets_close_character_operands_before_operations_and_negation() {
        for pattern in ["\\P{Lowercase_Letter}", "[\\P{Lowercase_Letter}]", "[^\\p{Lowercase_Letter}]"] {
            for input in ["a", "A"] {
                assert!(re(pattern, "iv").exec(&input.into()).is_none(), "{pattern} / {input}");
            }
            assert!(re(pattern, "iv").exec(&"1".into()).is_some());
        }
        for input in ["a", "A"] {
            assert!(re("[[a]&&[A]]", "iv").exec(&input.into()).is_some());
            assert!(re("[[a]--[A]]", "iv").exec(&input.into()).is_none());
            assert!(re("[a&&\\q{A}]", "iv").exec(&input.into()).is_some());
            assert!(re("[a--\\q{A}]", "iv").exec(&input.into()).is_none());
        }
        for input in ["ſ", "K"] {
            assert!(re("[\\W]", "iu").exec(&input.into()).is_none());
            assert!(re("[\\W]", "iv").exec(&input.into()).is_none());
        }
        assert!(re("[\\b]", "v").exec(&"b".into()).is_none());
        assert!(re("[\\b]", "v").exec(&"\u{8}".into()).is_some());
    }

    #[test]
    fn q_sets_keep_empty_alternatives_and_raw_multicharacter_operation_values() {
        let mut empty = re("[\\q{|a}]", "gv");
        assert_eq!(empty.exec(&"".into()).unwrap().captures[0], Some("".into()));
        assert_eq!(empty.last_index(), 0.0);
        assert_eq!(re("[\\q{|a}]", "v").exec(&"a".into()).unwrap().captures[0], Some("a".into()));
        assert_eq!(re("[\\q{|ab}c]", "v").exec(&"c".into()).unwrap().captures[0], Some("c".into()));
        assert_eq!(re("[\\q{|a}]a", "v").exec(&"a".into()).unwrap().captures[0], Some("a".into()));
        for input in ["a", "&", "b"] {
            assert_eq!(re("[a&b]", "v").exec(&input.into()).unwrap().captures[0], Some(input.into()));
        }
        assert!(re("[^\\q{a}]", "iv").exec(&"A".into()).is_none());
        assert!(re("[^a--\\q{ab}]", "v").exec(&"b".into()).is_some());
        for pattern in ["[^\\q{|a}]", "[^\\q{ab}&&a]", "[^[\\q{ab}]]"] {
            let error = JsRegExp::new(pattern.into(), "v").unwrap_err();
            assert_eq!(error.message, "Invalid regular expression: negated class set may contain strings");
        }
        for input in ["ab", "AB"] {
            assert!(re("[\\q{aB}&&\\q{Ab}]", "iv").exec(&input.into()).is_none());
            assert!(re("[\\q{aB}--\\q{Ab}]", "iv").exec(&input.into()).is_some());
        }
    }

    #[test]
    fn stateful_failure_resets_and_empty_exec_does_not_advance() {
        let mut global = re("a", "g");
        assert_eq!(global.exec(&"ba".into()).unwrap().index, 1);
        assert_eq!(global.last_index(), 2.0);
        assert!(global.exec(&"ba".into()).is_none());
        assert_eq!(global.last_index(), 0.0);
        let mut sticky = re("a", "y");
        assert!(sticky.exec(&"ba".into()).is_none());
        sticky.set_last_index(1.0);
        assert_eq!(sticky.exec(&"ba".into()).unwrap().index, 1);
        for flags in ["", "g", "y"] {
            let mut empty = re("()", flags);
            empty.set_last_index(1.0);
            assert_eq!(empty.exec(&"ab".into()).unwrap().index, usize::from(!flags.is_empty()));
            assert_eq!(empty.last_index(), 1.0);
            assert!(empty.exec(&"ab".into()).is_some());
            assert_eq!(empty.last_index(), 1.0);
        }
    }

    #[test]
    fn last_index_uses_to_length_only_for_stateful_exec() {
        let input = WireString::from("ba");
        for start in [-1.0, 0.5, 1.5, f64::NAN] {
            let mut regex = re("a", "g");
            regex.set_last_index(start);
            assert_eq!(regex.exec(&input).unwrap().index, 1);
            assert_eq!(regex.last_index(), 2.0);
        }
        let mut regex = re("a", "g");
        regex.set_last_index(f64::INFINITY);
        assert!(regex.exec(&input).is_none());
        assert_eq!(regex.last_index(), 0.0);
        let mut ordinary = re("a", "");
        ordinary.set_last_index(-0.0);
        assert!(ordinary.exec(&input).is_some());
        assert_eq!(ordinary.last_index().to_bits(), (-0.0_f64).to_bits());
        ordinary.set_last_index(f64::NAN);
        assert!(ordinary.exec(&input).is_some());
        assert!(ordinary.last_index().is_nan());
    }

    #[test]
    fn captures_keep_undefined_empty_and_named_indices() {
        let mut regex = re("(?<name>a)?()b", "d");
        let matched = regex.exec(&"b".into()).unwrap();
        assert_eq!(matched.captures, vec![Some("b".into()), None, Some("".into())]);
        assert_eq!(matched.groups, Some(vec![("name".into(), None)]));
        assert_eq!(matched.indices, Some(vec![Some(0..1), None, Some(0..0)]));
        assert_eq!(matched.group_indices, Some(vec![("name".into(), None)]));
    }

    #[test]
    fn lookbehind_backreferences_and_unicode_sets_use_the_native_engine() {
        assert_eq!(re("(?<=x)(y)\\1", "").exec(&"xyy".into()).unwrap().captures[1], Some("y".into()));
        let mut sets = re("([[a-z]&&[^aeiou]])", "v");
        assert_eq!(sets.exec(&"ab".into()).unwrap().captures[1], Some("b".into()));
    }
}
