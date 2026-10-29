//! Fixed OMP 596f2da, packages/utils/src/json-parse.ts and json-lexer.ts (MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See LICENSE.
//!
//! Final and dialect streaming parsing share this grammar. Ordinary Provider
//! strict arguments keep their existing contracts. Object writes are routed
//! through the one shared value carrier: relaxed `out[key] = value` must preserve
//! inherited __proto__ setters, while strict JSON.parse creates own data keys.

use ara_rpc::value_heap::StringifyMode;
use ara_rpc::{WireString, WireValue};
use std::fmt;

/// Implemented by Root's shared full-value bridge. There is no production
/// serde/flat-map fallback implementation. All constructors must retain f64 and
/// UTF-16 code units, and cloned values must retain required object identity.
pub trait FinalValueFactory {
    type Value: Clone;
    type Error;
    /// Generic runtime errors represent Native exceptions by default. Bindings
    /// that also report unsupported Host/carrier boundaries must classify those
    /// as false so a Native catch cannot turn missing support into a success.
    fn should_catch_runtime_error(&self, _error: &Self::Error) -> bool {
        true
    }
    /// Import strict JSON own properties, including own __proto__ data keys.
    /// WireValue preserves JS f64 overflow and unpaired UTF-16 surrogates.
    fn import_strict(&self, value: WireValue) -> Result<Self::Value, Self::Error>;
    fn null(&self) -> Result<Self::Value, Self::Error>;
    fn boolean(&self, value: bool) -> Result<Self::Value, Self::Error>;
    fn number(&self, value: f64) -> Result<Self::Value, Self::Error>;
    fn string(&self, value: WireString) -> Result<Self::Value, Self::Error>;
    /// Allocate before parsing the first element, then use ordinary push.
    fn array(&self) -> Result<Self::Value, Self::Error>;
    fn append(&self, array: &mut Self::Value, value: Self::Value) -> Result<(), Self::Error>;
    fn object(&self) -> Result<Self::Value, Self::Error>;
    /// Ordinary assignment, not define-own-property or key deletion. In
    /// particular repeated __proto__ after a null prototype can become own data.
    fn assign(&self, object: &mut Self::Value, key: WireString, value: Self::Value) -> Result<(), Self::Error>;
    /// Native asRecord: non-null object, excluding arrays.
    fn is_record(&self, value: &Self::Value) -> Result<bool, Self::Error>;
    /// Truthiness when an ordinary fresh record looks up an inherited member.
    /// Native NON_RECOVERABLE_BAREWORDS is an ordinary record and uses indexed
    /// lookup, so inherited names (e.g. constructor/toString/__proto__) also
    /// fail recovery; an own-only HashSet is not its whole lookup contract.
    /// The receiver is the fresh record, not Object.prototype itself: its
    /// inherited __proto__ getter returns Object.prototype (truthy), not null.
    fn ordinary_prototype_lookup_truthy(&self, key: &WireString) -> Result<bool, Self::Error>;
    /// Actual property Get, including primitive boxing and inherited accessors.
    fn get(&self, value: &Self::Value, key: &WireString) -> Result<Self::Value, Self::Error>;
    /// Primitive typeof-string only; no coercion or boxed-string unboxing.
    fn string_value(&self, value: &Self::Value) -> Result<Option<WireString>, Self::Error>;
    /// Native strict equality to Number(0), including -0; never coercion.
    fn is_number_zero(&self, value: &Self::Value) -> Result<bool, Self::Error>;
    fn is_null(&self, value: &Self::Value) -> Result<bool, Self::Error>;
    /// Fresh object-literal property definition, distinct from ordinary assign.
    fn define_own_data(&self, object: &mut Self::Value, key: WireString, value: Self::Value)
    -> Result<(), Self::Error>;
    fn stringify_json(&self, value: &Self::Value, mode: StringifyMode) -> Result<Option<WireString>, Self::Error>;
    /// Template-literal ToString: unlike explicit String, direct Symbol throws.
    fn implicit_string(&self, value: &Self::Value) -> Result<WireString, Self::Error>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalJsonError {
    pub message: String,
    /// Offset into Native UTF-16 source units.
    pub position: usize,
}

impl fmt::Display for FinalJsonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}
impl std::error::Error for FinalJsonError {}

/// Grammar failure can trigger repair. An actual carrier/runtime failure cannot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinalParseError<E> {
    Syntax(FinalJsonError),
    Runtime(E),
}

impl<E> FinalParseError<E> {
    pub fn syntax(&self) -> Option<&FinalJsonError> {
        match self {
            Self::Syntax(error) => Some(error),
            Self::Runtime(_) => None,
        }
    }
}
impl<E> From<FinalJsonError> for FinalParseError<E> {
    fn from(error: FinalJsonError) -> Self {
        Self::Syntax(error)
    }
}
impl<E: fmt::Display> fmt::Display for FinalParseError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax(error) => fmt::Display::fmt(error, formatter),
            Self::Runtime(error) => fmt::Display::fmt(error, formatter),
        }
    }
}
impl<E: std::error::Error + 'static> std::error::Error for FinalParseError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Syntax(error) => Some(error),
            Self::Runtime(error) => Some(error),
        }
    }
}

/// Native strict JSON.parse fast path, then final RelaxedJson(source, false).
/// A strict number overflowing f64 remains Infinity; it is not rejected by the
/// relaxed-number finite guard that only applies after strict parsing failed.
pub fn parse_json_with_repair<F: FinalValueFactory>(
    source: &str,
    factory: &F,
) -> Result<F::Value, FinalParseError<F::Error>> {
    parse_json_with_repair_units(&WireString::from(source), factory)
}

pub fn parse_relaxed_final<F: FinalValueFactory>(
    source: &str,
    factory: &F,
) -> Result<F::Value, FinalParseError<F::Error>> {
    parse_relaxed_final_units(&WireString::from(source), factory)
}

/// Root supplies the one strict grammar's UTF-16 input adaptation. Never route
/// raw unpaired source units straight to relaxed: strict own-data and overflow
/// semantics must remain the first attempt in the full source domain.
pub fn parse_json_with_repair_units<F: FinalValueFactory>(
    source: &WireString,
    factory: &F,
) -> Result<F::Value, FinalParseError<F::Error>> {
    if let Ok(value) = WireValue::parse_units(source.units()) {
        return factory.import_strict(value).map_err(FinalParseError::Runtime);
    }
    parse_relaxed_final_units(source, factory)
}

pub fn parse_relaxed_final_units<F: FinalValueFactory>(
    source: &WireString,
    factory: &F,
) -> Result<F::Value, FinalParseError<F::Error>> {
    let mut parser = Parser { source: source.units(), position: 0, factory, partial: false };
    parser.whitespace();
    if parser.at_end() {
        return Err(parser.error("Unexpected end of JSON input").into());
    }
    let value = parser.value(false)?.ok_or_else(|| parser.error("Unexpected end of JSON input"))?;
    parser.whitespace();
    if !parser.at_end() {
        return Err(parser.error(&format!("Unexpected trailing characters at position {}", parser.position)).into());
    }
    Ok(value)
}

/// Fixed json-parse.ts:309–320. Strict grammar success is imported once;
/// runtime import errors never retry through the relaxed grammar.
pub fn parse_streaming_json_units<F: FinalValueFactory>(
    source: &WireString,
    factory: &F,
) -> Result<F::Value, FinalParseError<F::Error>> {
    let start = source
        .units()
        .iter()
        .position(|&unit| !char::from_u32(u32::from(unit)).is_some_and(ara_prompt::js::is_space))
        .unwrap_or(source.len());
    let source = &source.units()[start..];
    if source.is_empty() {
        return factory.object().map_err(FinalParseError::Runtime);
    }
    if let Ok(strict) = WireValue::parse_units(source) {
        return factory.import_strict(strict).map_err(FinalParseError::Runtime);
    }
    // Coalescing allocation belongs to this inner try. The catch-body
    // allocation below escapes that catch if it fails.
    let partial = (|| {
        let mut parser = Parser { source, position: 0, factory, partial: true };
        parser.whitespace();
        let value = if parser.at_end() { None } else { parser.value(false)? };
        parser.whitespace(); // partial mode deliberately ignores trailing input
        match value {
            Some(value) if !factory.is_null(&value).map_err(FinalParseError::Runtime)? => Ok(value),
            _ => factory.object().map_err(FinalParseError::Runtime),
        }
    })();
    match partial {
        Ok(value) => Ok(value),
        Err(FinalParseError::Syntax(_)) => factory.object().map_err(FinalParseError::Runtime),
        Err(FinalParseError::Runtime(error)) if factory.should_catch_runtime_error(&error) => {
            factory.object().map_err(FinalParseError::Runtime)
        }
        Err(error) => Err(error),
    }
}

struct Parser<'a, F: FinalValueFactory> {
    source: &'a [u16],
    position: usize,
    factory: &'a F,
    partial: bool,
}

impl<F: FinalValueFactory> Parser<'_, F> {
    fn at_end(&self) -> bool {
        self.position >= self.source.len()
    }
    fn peek(&self) -> Option<u16> {
        self.source.get(self.position).copied()
    }
    fn error(&self, message: &str) -> FinalJsonError {
        FinalJsonError { message: message.into(), position: self.position }
    }

    fn insignificant_from(&self, mut position: usize) -> usize {
        let length = self.source.len();
        loop {
            while position < length && is_whitespace(self.source[position]) {
                position += 1;
            }
            if position + 1 < length && self.source[position] == 0x2f {
                if self.source[position + 1] == 0x2f {
                    position += 2;
                    while position < length && self.source[position] != 0x0a {
                        position += 1;
                    }
                    continue;
                }
                if self.source[position + 1] == 0x2a {
                    position += 2;
                    while position + 1 < length && !(self.source[position] == 0x2a && self.source[position + 1] == 0x2f)
                    {
                        position += 1;
                    }
                    position = (position + 2).min(length);
                    continue;
                }
            }
            return position;
        }
    }

    fn whitespace(&mut self) {
        self.position = self.insignificant_from(self.position);
    }

    // None is the Native private INCOMPLETE sentinel, never an allocated value.
    fn value(&mut self, allow_bareword: bool) -> Result<Option<F::Value>, FinalParseError<F::Error>> {
        match self.peek() {
            Some(0x7b) => return self.object().map(Some),
            Some(0x5b) => return self.array().map(Some),
            Some(quote @ (0x22 | 0x27)) => {
                let value = self.string(quote)?;
                return self.factory.string(value).map(Some).map_err(FinalParseError::Runtime);
            }
            Some(unit) if is_number_start(unit) => {
                let value = match self.number() {
                    Ok(value) => value,
                    Err(_) if self.partial => return Ok(None),
                    Err(error) => return Err(error.into()),
                };
                return self.factory.number(value).map(Some).map_err(FinalParseError::Runtime);
            }
            _ => {}
        }
        if let Some(value) = self.keyword() {
            return value.map(Some).map_err(FinalParseError::Runtime);
        }
        if self.partial {
            self.position = self.source.len();
            return Ok(None);
        }
        if allow_bareword {
            let word = self.bareword()?;
            return self.factory.string(word).map(Some).map_err(FinalParseError::Runtime);
        }
        Err(self.error(&format!("Unexpected token at position {}", self.position)).into())
    }

    fn object(&mut self) -> Result<F::Value, FinalParseError<F::Error>> {
        self.position += 1;
        let mut object = self.factory.object().map_err(FinalParseError::Runtime)?;
        loop {
            self.whitespace();
            match self.peek() {
                None if self.partial => return Ok(object),
                None => return Err(self.error("Unterminated object").into()),
                Some(0x7d) => {
                    self.position += 1;
                    return Ok(object);
                }
                Some(0x2c) => {
                    self.position += 1;
                    continue;
                }
                _ => {}
            }
            let key = self.key()?;
            self.whitespace();
            if self.peek() != Some(0x3a) {
                if self.partial {
                    return Ok(object);
                }
                return Err(self.error("Expected ':' in object").into());
            }
            self.position += 1;
            self.whitespace();
            if self.at_end() {
                if self.partial {
                    return Ok(object);
                }
                return Err(self.error("Expected value after ':'").into());
            }
            let Some(value) = self.value(true)? else {
                return Ok(object);
            };
            self.factory.assign(&mut object, key, value).map_err(FinalParseError::Runtime)?;
            self.whitespace();
            match self.peek() {
                Some(0x2c) => self.position += 1,
                Some(0x7d) => {
                    self.position += 1;
                    return Ok(object);
                }
                _ if self.partial => return Ok(object),
                _ => return Err(self.error("Expected ',' or '}' in object").into()),
            }
        }
    }

    fn array(&mut self) -> Result<F::Value, FinalParseError<F::Error>> {
        self.position += 1;
        let mut array = self.factory.array().map_err(FinalParseError::Runtime)?;
        loop {
            self.whitespace();
            match self.peek() {
                None if self.partial => return Ok(array),
                None => return Err(self.error("Unterminated array").into()),
                Some(0x5d) => {
                    self.position += 1;
                    return Ok(array);
                }
                Some(0x2c) => {
                    self.position += 1;
                    continue;
                }
                _ => {}
            }
            let Some(value) = self.value(true)? else {
                return Ok(array);
            };
            self.factory.append(&mut array, value).map_err(FinalParseError::Runtime)?;
            self.whitespace();
            match self.peek() {
                Some(0x2c) => self.position += 1,
                Some(0x5d) => {
                    self.position += 1;
                    return Ok(array);
                }
                _ if self.partial => return Ok(array),
                _ => return Err(self.error("Expected ',' or ']' in array").into()),
            }
        }
    }

    fn key(&mut self) -> Result<WireString, FinalJsonError> {
        if let Some(quote @ (0x22 | 0x27)) = self.peek() {
            return self.string(quote);
        }
        let start = self.position;
        while let Some(unit) = self.peek() {
            if matches!(unit, 0x3a | 0x2c | 0x7d) || is_whitespace(unit) {
                break;
            }
            self.position += 1;
        }
        if self.position == start && !self.partial {
            return Err(self.error("Expected object key"));
        }
        Ok(WireString::from_units(self.source[start..self.position].to_vec()))
    }

    fn string(&mut self, quote: u16) -> Result<WireString, FinalJsonError> {
        let mut cursor = self.position + 1;
        let mut out = Vec::new();
        while cursor < self.source.len() {
            let unit = self.source[cursor];
            if unit == quote {
                let look = self.insignificant_from(cursor + 1);
                if (quote == 0x22 && !self.partial)
                    || look >= self.source.len()
                    || matches!(self.source[look], 0x2c | 0x7d | 0x5d | 0x3a)
                {
                    self.position = cursor + 1;
                    return Ok(WireString::from_units(out));
                }
                // A single quote followed by ordinary content is an apostrophe.
                out.push(unit);
                cursor += 1;
                continue;
            }
            if unit != 0x5c {
                out.push(unit);
                cursor += 1;
                continue;
            }
            cursor += 1;
            let Some(&escaped) = self.source.get(cursor) else {
                if self.partial {
                    out.push(0x5c);
                }
                break;
            };
            match escaped {
                0x22 | 0x27 | 0x5c | 0x2f => out.push(escaped),
                0x62 => out.push(0x08),
                0x66 => out.push(0x0c),
                0x6e => out.push(0x0a),
                0x72 => out.push(0x0d),
                0x74 => out.push(0x09),
                0x75 => {
                    if let Some(unit) = hex4(self.source, cursor + 1) {
                        out.push(unit);
                        cursor += 4;
                    } else {
                        // Invalid/truncated escape is literal; the later quote
                        // still has to close the final string.
                        out.extend([0x5c, 0x75]);
                    }
                }
                _ => out.extend([0x5c, escaped]),
            }
            cursor += 1;
        }
        if self.partial {
            self.position = cursor;
            Ok(WireString::from_units(out))
        } else {
            Err(self.error("Unterminated string"))
        }
    }

    fn number(&mut self) -> Result<f64, FinalJsonError> {
        let start = self.position;
        while self.peek().is_some_and(is_number_char) {
            self.position += 1;
        }
        let token = String::from_utf16(&self.source[start..self.position]).expect("numeric token is ASCII");
        if let Some(value) = js_number_token(&token).filter(|value| value.is_finite()) {
            return Ok(value);
        }
        Err(self.error(&format!("Invalid number: {token}")))
    }

    fn keyword(&mut self) -> Option<Result<F::Value, F::Error>> {
        for (word, kind) in [("true", 1), ("false", 0), ("null", 2), ("True", 1), ("False", 0), ("None", 2)] {
            let length = word.len();
            if self
                .source
                .get(self.position..self.position + length)
                .is_some_and(|slice| slice.iter().zip(word.bytes()).all(|(&unit, byte)| unit == u16::from(byte)))
                && !self.source.get(self.position + length).is_some_and(|&unit| is_ident_char(unit))
            {
                self.position += length;
                return Some(if kind == 2 { self.factory.null() } else { self.factory.boolean(kind == 1) });
            }
        }
        None
    }

    fn bareword(&mut self) -> Result<WireString, FinalParseError<F::Error>> {
        let start = self.position;
        let mut cursor = start;
        while let Some(&unit) = self.source.get(cursor) {
            if matches!(unit, 0x2c | 0x7d | 0x5d | 0x0a | 0x0d) {
                break;
            }
            if matches!(unit, 0x22 | 0x7b | 0x5b)
                || unit == 0x3a && !self.source.get(cursor + 1).is_some_and(|&next| matches!(next, 0x2f | 0x5c))
            {
                return Err(self.error(&format!("Unexpected token at position {start}")).into());
            }
            cursor += 1;
        }
        if cursor >= self.source.len() {
            return Err(self.error(&format!("Unexpected token at position {start}")).into());
        }
        let mut end = cursor;
        while end > start && is_whitespace(self.source[end - 1]) {
            end -= 1;
        }
        let word = WireString::from_units(self.source[start..end].to_vec());
        if ["NaN", "Infinity", "-Infinity", "+Infinity", "undefined"].iter().any(|value| word.equals_ascii(value))
            || self.factory.ordinary_prototype_lookup_truthy(&word).map_err(FinalParseError::Runtime)?
        {
            return Err(self.error(&format!("Unexpected token at position {start}")).into());
        }
        self.position = cursor;
        Ok(word)
    }
}

fn is_whitespace(unit: u16) -> bool {
    matches!(unit, 0x20 | 0x09 | 0x0a | 0x0d)
}
fn is_ident_char(unit: u16) -> bool {
    (0x30..=0x39).contains(&unit) || (0x61..=0x7a).contains(&(unit | 0x20)) || matches!(unit, 0x5f | 0x24)
}
fn is_number_start(unit: u16) -> bool {
    matches!(unit, 0x2d | 0x2b | 0x2e) || (0x30..=0x39).contains(&unit)
}
fn is_number_char(unit: u16) -> bool {
    is_number_start(unit) || (0x61..=0x66).contains(&(unit | 0x20)) || (unit | 0x20) == 0x78
}
fn hex_digit(unit: u16) -> Option<u16> {
    match unit {
        0x30..=0x39 => Some(unit - 0x30),
        _ if (0x61..=0x66).contains(&(unit | 0x20)) => Some((unit | 0x20) - 0x61 + 10),
        _ => None,
    }
}
fn hex4(source: &[u16], position: usize) -> Option<u16> {
    source
        .get(position..position + 4)?
        .iter()
        .try_fold(0, |value, &unit| hex_digit(unit).map(|digit| (value << 4) | digit))
}

fn js_number_token(token: &str) -> Option<f64> {
    let (digits, bits_per_digit) = if token.starts_with("0x") || token.starts_with("0X") {
        (&token[2..], 4usize)
    } else if token.starts_with("0b") || token.starts_with("0B") {
        (&token[2..], 1usize)
    } else {
        return token.parse::<f64>().ok();
    };
    if digits.is_empty() {
        return None;
    }
    let digits: Vec<u8> = digits
        .bytes()
        .map(|byte| hex_digit(u16::from(byte)).and_then(|value| (value < (1 << bits_per_digit)).then_some(value as u8)))
        .collect::<Option<_>>()?;
    let Some(first) = digits.iter().position(|&digit| digit != 0) else {
        return Some(0.0);
    };
    let leading_bits = (8 - digits[first].leading_zeros()) as usize;
    let bit_length = leading_bits + (digits.len() - first - 1) * bits_per_digit;
    if bit_length > 1024 {
        return Some(f64::INFINITY);
    }
    let bit = |index: usize| -> u8 {
        if index < leading_bits {
            return (digits[first] >> (leading_bits - index - 1)) & 1;
        }
        let rest = index - leading_bits;
        (digits[first + 1 + rest / bits_per_digit] >> (bits_per_digit - rest % bits_per_digit - 1)) & 1
    };
    let kept = bit_length.min(53);
    let mut mantissa = (0..kept).fold(0u64, |value, index| (value << 1) | u64::from(bit(index)));
    if bit_length > 53 && bit(53) == 1 {
        let sticky = (54..bit_length).any(|index| bit(index) != 0);
        if sticky || mantissa & 1 == 1 {
            mantissa += 1;
        }
    }
    Some(mantissa as f64 * 2.0f64.powi((bit_length - kept) as i32))
}
