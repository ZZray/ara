//! JSON values as ECMAScript's wire representation sees them.
//!
//! Rust strings cannot represent unpaired UTF-16 surrogates. RPC framing uses
//! JavaScript string slicing and JSON.stringify, so this private representation
//! retains code units until the final UTF-8 encoding.

use std::{collections::HashMap, fmt};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WireString(Vec<u16>);

impl From<&str> for WireString {
    fn from(value: &str) -> Self {
        Self(value.encode_utf16().collect())
    }
}

impl From<String> for WireString {
    fn from(value: String) -> Self {
        Self::from(value.as_str())
    }
}

impl WireString {
    /// Retain UTF-16 code units, including unpaired surrogates, without replacement.
    pub fn from_units(units: Vec<u16>) -> Self {
        Self(units)
    }

    pub fn units(&self) -> &[u16] {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn slice_prefix(&self, len: usize) -> Self {
        Self(self.0[..len.min(self.0.len())].to_vec())
    }

    pub fn append_str(&mut self, suffix: &str) {
        self.0.extend(suffix.encode_utf16());
    }

    pub fn equals_ascii(&self, ascii: &str) -> bool {
        ascii.is_ascii()
            && self.0.len() == ascii.len()
            && self.0.iter().zip(ascii.bytes()).all(|(&unit, byte)| unit == u16::from(byte))
    }

    pub fn to_utf8(&self) -> Result<String, std::string::FromUtf16Error> {
        String::from_utf16(&self.0)
    }
}

#[derive(Debug, PartialEq)]
pub enum WireValue {
    Null,
    Bool(bool),
    Number(f64),
    String(WireString),
    Array(Vec<WireValue>),
    Object(Vec<(WireString, WireValue)>),
}

impl WireValue {
    pub fn parse(input: &str) -> Result<Self, JsonError> {
        Parser::new(input).parse()
    }

    pub fn stringify(&self) -> String {
        enum Task<'a> {
            Value(&'a WireValue),
            Key(&'a WireString),
            Literal(&'static str),
        }

        let mut output = String::new();
        let mut tasks = vec![Task::Value(self)];
        let mut number_buffer = ryu_js::Buffer::new();
        while let Some(task) = tasks.pop() {
            match task {
                Task::Literal(text) => output.push_str(text),
                Task::Key(key) => write_string(&mut output, key),
                Task::Value(value) => match value {
                    Self::Null => output.push_str("null"),
                    Self::Bool(true) => output.push_str("true"),
                    Self::Bool(false) => output.push_str("false"),
                    Self::Number(number) if !number.is_finite() => output.push_str("null"),
                    Self::Number(number) if *number == 0.0 => output.push('0'),
                    Self::Number(number) => output.push_str(number_buffer.format_finite(*number)),
                    Self::String(value) => write_string(&mut output, value),
                    Self::Array(items) => {
                        output.push('[');
                        tasks.push(Task::Literal("]"));
                        for (index, item) in items.iter().enumerate().rev() {
                            tasks.push(Task::Value(item));
                            if index != 0 {
                                tasks.push(Task::Literal(","));
                            }
                        }
                    }
                    Self::Object(_) => {
                        output.push('{');
                        tasks.push(Task::Literal("}"));
                        let entries = value.entries().expect("object entries");
                        for (index, (key, item)) in entries.into_iter().enumerate().rev() {
                            tasks.push(Task::Value(item));
                            tasks.push(Task::Literal(":"));
                            tasks.push(Task::Key(key));
                            if index != 0 {
                                tasks.push(Task::Literal(","));
                            }
                        }
                    }
                },
            }
        }
        output
    }

    pub fn object(entries: Vec<(&str, WireValue)>) -> Self {
        let mut result = Self::Object(Vec::new());
        for (key, value) in entries {
            result.insert(key, value);
        }
        result
    }

    pub fn get(&self, key: &str) -> Option<&WireValue> {
        let Self::Object(entries) = self else { return None };
        entries.iter().find(|(candidate, _)| candidate.0.iter().copied().eq(key.encode_utf16())).map(|(_, value)| value)
    }

    pub fn insert(&mut self, key: &str, value: WireValue) {
        let Self::Object(entries) = self else { return };
        insert_entry(entries, WireString::from(key), value);
    }

    /// Own property entries in JavaScript's Object.keys order.
    pub fn entries(&self) -> Option<Vec<(&WireString, &WireValue)>> {
        let Self::Object(entries) = self else { return None };
        let mut indices = Vec::new();
        let mut ordinary = Vec::new();
        for (key, value) in entries {
            if let Some(index) = array_index(key) {
                indices.push((index, key, value));
            } else {
                ordinary.push((key, value));
            }
        }
        indices.sort_unstable_by_key(|(index, _, _)| *index);
        let mut ordered = Vec::with_capacity(entries.len());
        ordered.extend(indices.into_iter().map(|(_, key, value)| (key, value)));
        ordered.extend(ordinary);
        Some(ordered)
    }

    pub fn as_array(&self) -> Option<&[WireValue]> {
        match self {
            Self::Array(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            Self::Number(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_string(&self) -> Option<&WireString> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn is_object(&self) -> bool {
        matches!(self, Self::Object(_))
    }

    /// Structural comparison of JSON values; object insertion order is ignored.
    pub fn deep_equal(&self, other: &Self) -> bool {
        let mut pending = vec![(self, other)];
        while let Some((left, right)) = pending.pop() {
            match (left, right) {
                (Self::Null, Self::Null) => {}
                (Self::Bool(a), Self::Bool(b)) if a == b => {}
                (Self::Number(a), Self::Number(b)) if a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan()) => {}
                (Self::String(a), Self::String(b)) if a == b => {}
                (Self::Array(a), Self::Array(b)) if a.len() == b.len() => {
                    pending.extend(a.iter().zip(b));
                }
                (Self::Object(a), Self::Object(b)) if a.len() == b.len() => {
                    let by_key: HashMap<&WireString, &WireValue> = b.iter().map(|(key, value)| (key, value)).collect();
                    if by_key.len() != b.len() {
                        return false;
                    }
                    for (key, value) in a {
                        let Some(&peer) = by_key.get(key) else { return false };
                        pending.push((value, peer));
                    }
                }
                _ => return false,
            }
        }
        true
    }
}

impl Clone for WireValue {
    fn clone(&self) -> Self {
        enum Frame<'a> {
            Array(&'a [WireValue], usize, Vec<WireValue>),
            Object(&'a [(WireString, WireValue)], usize, Vec<(WireString, WireValue)>),
        }
        let mut stack: Vec<Frame<'_>> = Vec::new();
        let mut current = self;
        loop {
            let cloned = match current {
                Self::Null => Self::Null,
                Self::Bool(value) => Self::Bool(*value),
                Self::Number(value) => Self::Number(*value),
                Self::String(value) => Self::String(value.clone()),
                Self::Array(items) if items.is_empty() => Self::Array(Vec::new()),
                Self::Object(entries) if entries.is_empty() => Self::Object(Vec::new()),
                Self::Array(items) => {
                    stack.push(Frame::Array(items, 0, Vec::with_capacity(items.len())));
                    current = &items[0];
                    continue;
                }
                Self::Object(entries) => {
                    stack.push(Frame::Object(entries, 0, Vec::with_capacity(entries.len())));
                    current = &entries[0].1;
                    continue;
                }
            };
            let mut completed = cloned;
            loop {
                let Some(frame) = stack.last_mut() else { return completed };
                match frame {
                    Frame::Array(items, index, values) => {
                        values.push(completed);
                        *index += 1;
                        if *index < items.len() {
                            current = &items[*index];
                            break;
                        }
                        completed = Self::Array(std::mem::take(values));
                    }
                    Frame::Object(entries, index, values) => {
                        values.push((entries[*index].0.clone(), completed));
                        *index += 1;
                        if *index < entries.len() {
                            current = &entries[*index].1;
                            break;
                        }
                        completed = Self::Object(std::mem::take(values));
                    }
                }
                stack.pop();
            }
        }
    }
}

impl Drop for WireValue {
    fn drop(&mut self) {
        let mut pending = Vec::new();
        match self {
            Self::Array(items) => pending.extend(std::mem::take(items)),
            Self::Object(entries) => pending.extend(std::mem::take(entries).into_iter().map(|(_, value)| value)),
            _ => return,
        }
        while let Some(mut value) = pending.pop() {
            match &mut value {
                Self::Array(items) => pending.extend(std::mem::take(items)),
                Self::Object(entries) => pending.extend(std::mem::take(entries).into_iter().map(|(_, value)| value)),
                _ => {}
            }
        }
    }
}

fn insert_entry(entries: &mut Vec<(WireString, WireValue)>, key: WireString, value: WireValue) {
    if let Some((_, existing)) = entries.iter_mut().find(|(candidate, _)| candidate == &key) {
        *existing = value;
    } else {
        entries.push((key, value));
    }
}

fn array_index(key: &WireString) -> Option<u32> {
    if key.is_empty() || key.len() > 10 || (key.len() > 1 && key.0[0] == b'0' as u16) {
        return None;
    }
    let mut index = 0u32;
    for &unit in key.units() {
        if !(b'0' as u16..=b'9' as u16).contains(&unit) {
            return None;
        }
        index = index.checked_mul(10)?.checked_add(u32::from(unit - b'0' as u16))?;
    }
    (index != u32::MAX).then_some(index)
}

fn write_string(output: &mut String, value: &WireString) {
    output.push('"');
    let units = value.units();
    let mut index = 0;
    while index < units.len() {
        let unit = units[index];
        match unit {
            0x22 => output.push_str("\\\""),
            0x5c => output.push_str("\\\\"),
            0x08 => output.push_str("\\b"),
            0x09 => output.push_str("\\t"),
            0x0a => output.push_str("\\n"),
            0x0c => output.push_str("\\f"),
            0x0d => output.push_str("\\r"),
            0x00..=0x1f => write_hex_escape(output, unit),
            0xd800..=0xdbff if index + 1 < units.len() && (0xdc00..=0xdfff).contains(&units[index + 1]) => {
                let codepoint = 0x10000 + (u32::from(unit - 0xd800) << 10) + u32::from(units[index + 1] - 0xdc00);
                output.push(char::from_u32(codepoint).expect("valid surrogate pair"));
                index += 1;
            }
            0xd800..=0xdfff => write_hex_escape(output, unit),
            _ => output.push(char::from_u32(u32::from(unit)).expect("valid BMP code point")),
        }
        index += 1;
    }
    output.push('"');
}

fn write_hex_escape(output: &mut String, unit: u16) {
    output.push_str("\\u");
    for shift in [12, 8, 4, 0] {
        let digit = ((unit >> shift) & 0xf) as u8;
        output.push(char::from(if digit < 10 { b'0' + digit } else { b'a' + digit - 10 }));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonError {
    pub offset: usize,
    pub message: &'static str,
}

impl fmt::Display for JsonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "JSON at byte {}: {}", self.offset, self.message)
    }
}

impl std::error::Error for JsonError {}

struct Parser<'a> {
    input: &'a str,
    position: usize,
}

enum Frame {
    Array {
        items: Vec<WireValue>,
        state: ArrayState,
    },
    Object {
        entries: Vec<(WireString, WireValue)>,
        indices: HashMap<WireString, usize>,
        key: Option<WireString>,
        state: ObjectState,
    },
}

#[derive(Clone, Copy)]
enum ArrayState {
    First,
    Value,
    Next,
}

#[derive(Clone, Copy)]
enum ObjectState {
    First,
    Key,
    Colon,
    Value,
    Next,
}

#[derive(Clone, Copy)]
enum State {
    Array(ArrayState),
    Object(ObjectState),
}

enum Started {
    Value(WireValue),
    Frame(Frame),
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, position: 0 }
    }

    fn parse(mut self) -> Result<WireValue, JsonError> {
        let mut stack: Vec<Frame> = Vec::new();
        let mut pending: Option<WireValue> = None;
        let mut root: Option<WireValue> = None;
        loop {
            if let Some(value) = pending.take() {
                if let Some(parent) = stack.last_mut() {
                    match parent {
                        Frame::Array { items, state: ArrayState::Value } => {
                            items.push(value);
                            if let Frame::Array { state, .. } = parent {
                                *state = ArrayState::Next;
                            }
                        }
                        Frame::Object { entries, indices, key, state: ObjectState::Value } => {
                            let key = key.take().expect("object key");
                            if let Some(&index) = indices.get(&key) {
                                entries[index].1 = value;
                            } else {
                                indices.insert(key.clone(), entries.len());
                                entries.push((key, value));
                            }
                            if let Frame::Object { state, .. } = parent {
                                *state = ObjectState::Next;
                            }
                        }
                        _ => unreachable!("a parent must expect the completed child"),
                    }
                } else {
                    root = Some(value);
                }
                continue;
            }

            self.whitespace();
            if stack.is_empty() {
                if let Some(value) = root {
                    return if self.position == self.input.len() {
                        Ok(value)
                    } else {
                        Err(self.error("trailing input"))
                    };
                }
                self.start(&mut stack, &mut pending)?;
                continue;
            }

            let state = match stack.last().expect("nonempty") {
                Frame::Array { state, .. } => State::Array(*state),
                Frame::Object { state, .. } => State::Object(*state),
            };
            match state {
                State::Array(ArrayState::First) if self.consume(b']') => {
                    let Frame::Array { items, .. } = stack.pop().expect("array") else { unreachable!() };
                    pending = Some(WireValue::Array(items));
                }
                State::Array(ArrayState::First | ArrayState::Value) => {
                    if let Some(Frame::Array { state, .. }) = stack.last_mut() {
                        *state = ArrayState::Value;
                    }
                    self.start(&mut stack, &mut pending)?;
                }
                State::Array(ArrayState::Next) if self.consume(b']') => {
                    let Frame::Array { items, .. } = stack.pop().expect("array") else { unreachable!() };
                    pending = Some(WireValue::Array(items));
                }
                State::Array(ArrayState::Next) if self.consume(b',') => {
                    if let Some(Frame::Array { state, .. }) = stack.last_mut() {
                        *state = ArrayState::Value;
                    }
                }
                State::Array(ArrayState::Next) => return Err(self.error("expected ',' or ']'")),
                State::Object(ObjectState::First) if self.consume(b'}') => {
                    let Frame::Object { entries, .. } = stack.pop().expect("object") else { unreachable!() };
                    pending = Some(WireValue::Object(entries));
                }
                State::Object(ObjectState::First | ObjectState::Key) => {
                    if self.peek() != Some(b'"') {
                        return Err(self.error("expected object key"));
                    }
                    let key = self.string()?;
                    if let Some(Frame::Object { key: slot, state, .. }) = stack.last_mut() {
                        *slot = Some(key);
                        *state = ObjectState::Colon;
                    }
                }
                State::Object(ObjectState::Colon) => {
                    if !self.consume(b':') {
                        return Err(self.error("expected ':'"));
                    }
                    if let Some(Frame::Object { state, .. }) = stack.last_mut() {
                        *state = ObjectState::Value;
                    }
                }
                State::Object(ObjectState::Value) => self.start(&mut stack, &mut pending)?,
                State::Object(ObjectState::Next) if self.consume(b'}') => {
                    let Frame::Object { entries, .. } = stack.pop().expect("object") else { unreachable!() };
                    pending = Some(WireValue::Object(entries));
                }
                State::Object(ObjectState::Next) if self.consume(b',') => {
                    if let Some(Frame::Object { state, .. }) = stack.last_mut() {
                        *state = ObjectState::Key;
                    }
                }
                State::Object(ObjectState::Next) => return Err(self.error("expected ',' or '}'")),
            }
        }
    }

    fn start(&mut self, stack: &mut Vec<Frame>, pending: &mut Option<WireValue>) -> Result<(), JsonError> {
        let started = match self.peek() {
            Some(b'{') => {
                self.position += 1;
                Started::Frame(Frame::Object {
                    entries: Vec::new(),
                    indices: HashMap::new(),
                    key: None,
                    state: ObjectState::First,
                })
            }
            Some(b'[') => {
                self.position += 1;
                Started::Frame(Frame::Array { items: Vec::new(), state: ArrayState::First })
            }
            Some(b'"') => Started::Value(WireValue::String(self.string()?)),
            Some(b't') => {
                self.literal("true")?;
                Started::Value(WireValue::Bool(true))
            }
            Some(b'f') => {
                self.literal("false")?;
                Started::Value(WireValue::Bool(false))
            }
            Some(b'n') => {
                self.literal("null")?;
                Started::Value(WireValue::Null)
            }
            Some(b'-' | b'0'..=b'9') => Started::Value(WireValue::Number(self.number()?)),
            _ => return Err(self.error("expected JSON value")),
        };
        match started {
            Started::Value(value) => *pending = Some(value),
            Started::Frame(frame) => stack.push(frame),
        }
        Ok(())
    }

    fn string(&mut self) -> Result<WireString, JsonError> {
        debug_assert_eq!(self.peek(), Some(b'"'));
        self.position += 1;
        let mut units = Vec::new();
        loop {
            let Some(byte) = self.peek() else { return Err(self.error("unterminated string")) };
            if byte == b'"' {
                self.position += 1;
                return Ok(WireString(units));
            }
            if byte < 0x20 {
                return Err(self.error("unescaped control character"));
            }
            if byte == b'\\' {
                self.position += 1;
                let Some(escape) = self.peek() else { return Err(self.error("incomplete escape")) };
                self.position += 1;
                let unit = match escape {
                    b'"' => b'"' as u16,
                    b'\\' => b'\\' as u16,
                    b'/' => b'/' as u16,
                    b'b' => 0x08,
                    b'f' => 0x0c,
                    b'n' => 0x0a,
                    b'r' => 0x0d,
                    b't' => 0x09,
                    b'u' => {
                        units.push(self.hex_quad()?);
                        continue;
                    }
                    _ => return Err(self.error("invalid escape")),
                };
                units.push(unit);
                continue;
            }
            let character = self.input[self.position..].chars().next().expect("valid UTF-8 input");
            let mut buffer = [0u16; 2];
            units.extend_from_slice(character.encode_utf16(&mut buffer));
            self.position += character.len_utf8();
        }
    }

    fn hex_quad(&mut self) -> Result<u16, JsonError> {
        let mut unit = 0u16;
        for _ in 0..4 {
            let Some(byte) = self.peek() else { return Err(self.error("incomplete unicode escape")) };
            let digit = match byte {
                b'0'..=b'9' => byte - b'0',
                b'a'..=b'f' => byte - b'a' + 10,
                b'A'..=b'F' => byte - b'A' + 10,
                _ => return Err(self.error("invalid unicode escape")),
            };
            unit = (unit << 4) | u16::from(digit);
            self.position += 1;
        }
        Ok(unit)
    }

    fn number(&mut self) -> Result<f64, JsonError> {
        let start = self.position;
        self.consume(b'-');
        match self.peek() {
            Some(b'0') => self.position += 1,
            Some(b'1'..=b'9') => {
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.position += 1;
                }
            }
            _ => return Err(self.error("invalid number")),
        }
        if self.consume(b'.') {
            let fraction_start = self.position;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.position += 1;
            }
            if self.position == fraction_start {
                return Err(self.error("invalid fraction"));
            }
        }
        if self.consume(b'e') || self.consume(b'E') {
            if !self.consume(b'+') {
                self.consume(b'-');
            }
            let exponent_start = self.position;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.position += 1;
            }
            if self.position == exponent_start {
                return Err(self.error("invalid exponent"));
            }
        }
        self.input[start..self.position].parse::<f64>().map_err(|_| self.error("invalid number"))
    }

    fn literal(&mut self, text: &str) -> Result<(), JsonError> {
        if self.input[self.position..].starts_with(text) {
            self.position += text.len();
            Ok(())
        } else {
            Err(self.error("invalid literal"))
        }
    }

    fn whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.position += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.as_bytes().get(self.position).copied()
    }

    fn consume(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn error(&self, message: &'static str) -> JsonError {
        JsonError { offset: self.position, message }
    }
}
