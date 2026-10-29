//! Fixed OMP 596f2da7101178214aa27a753529d15e6b7ad91d, packages/ai/src/utils/validation.ts.
//! MIT: Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See THIRD_PARTY.md.
//! UTF16 String operations consumed by fixed validation.ts normalizers.
//! OMP 596f2da7101178214aa27a753529d15e6b7ad91d (MIT); see THIRD_PARTY.md.
//! Prototype dispatch stays in property.rs; these are the selected native bodies.

use super::*;

pub(super) fn ecma_whitespace(unit: u16) -> bool {
    matches!(unit, 0x09..=0x0d | 0x20 | 0xa0 | 0x1680 | 0x2000..=0x200a | 0x2028..=0x2029 | 0x202f | 0x205f | 0x3000 | 0xfeff)
}

pub(super) fn integer_or_infinity(number: f64) -> f64 {
    if number.is_nan() { 0.0 } else { number.trunc() }
}

impl ValueHeap {
    pub(super) fn implicit_text(
        self: &Arc<Self>,
        value: &ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<WireString> {
        if self.kind(value)? == ValueKind::Symbol {
            return Err(ValueError::TypeError("Cannot implicitly convert Symbol to string".into()));
        }
        self.to_string(value, runtime)
    }

    pub(super) fn string_method(
        self: &Arc<Self>,
        method: Intrinsic,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        // split's @@split lookup precedes ToString(receiver). Primitive source
        // separators have no such method; object separators stay observable.
        if method == Intrinsic::StringSplit {
            if matches!(self.kind(receiver)?, ValueKind::Undefined | ValueKind::Null) {
                return Err(ValueError::TypeError("String.split receiver is null or undefined".into()));
            }
            if let Some(separator) = arguments.first()
                && !matches!(self.kind(separator)?, ValueKind::Undefined | ValueKind::Null)
            {
                let split = self.get(separator, &self.split_key(), runtime)?;
                if !matches!(self.kind(&split)?, ValueKind::Undefined | ValueKind::Null) {
                    let limit = arguments.get(1).cloned().unwrap_or(self.undefined()?);
                    return self.call(&split, separator, &[receiver.clone(), limit], runtime);
                }
            }
        }
        let text = self.string_method_receiver(receiver, runtime)?;
        match method {
            // Fixed output-block.ts h.repeat(3)/h.repeat(fillCount). Keep the
            // ordinary prototype Get/Call and receiver conversion above; count
            // conversion follows it and preserves programmable coercion order.
            Intrinsic::StringRepeat => {
                let count = arguments.first().map(|v| self.to_number(v, runtime)).transpose()?.unwrap_or(f64::NAN);
                let count = integer_or_infinity(count);
                if count < 0.0 || !count.is_finite() {
                    return Err(ValueError::RangeError("Invalid repeat count".into()));
                }
                if count == 0.0 || text.is_empty() {
                    return self.string(WireString::from_units(Vec::new()));
                }
                // Match the existing StringPadStart UTF16 length boundary.
                // The allocation itself remains fallible and arena publication
                // still passes through ValueHeap::string.
                if count > (u32::MAX as usize / text.len()) as f64 {
                    return Err(ValueError::RangeError("Invalid string length".into()));
                }
                let count = count as usize;
                let length = text.len().checked_mul(count).ok_or_else(|| ValueError::RangeError("Invalid string length".into()))?;
                let mut units = Vec::new();
                units.try_reserve_exact(length).map_err(|_| ValueError::Allocation)?;
                for _ in 0..count {
                    units.extend_from_slice(text.units());
                }
                self.string(WireString::from_units(units))
            }
            Intrinsic::StringToWellFormed => {
                let mut output = Vec::new();
                output.try_reserve_exact(text.len()).map_err(|_| ValueError::Allocation)?;
                let mut index = 0usize;
                while index < text.len() {
                    let unit = text.units()[index];
                    if (0xd800..=0xdbff).contains(&unit) {
                        if let Some(&low) = text.units().get(index + 1)
                            && (0xdc00..=0xdfff).contains(&low)
                        {
                            output.extend_from_slice(&[unit, low]);
                            index += 2;
                            continue;
                        }
                        output.push(0xfffd);
                    } else if (0xdc00..=0xdfff).contains(&unit) {
                        output.push(0xfffd);
                    } else {
                        output.push(unit);
                    }
                    index += 1;
                }
                self.string(WireString::from_units(output))
            }
            Intrinsic::StringTrimStart | Intrinsic::StringTrimEnd => {
                let mut units = text.units();
                if method == Intrinsic::StringTrimStart {
                    while units.first().copied().is_some_and(ecma_whitespace) {
                        units = &units[1..];
                    }
                } else {
                    while units.last().copied().is_some_and(ecma_whitespace) {
                        units = &units[..units.len() - 1];
                    }
                }
                self.string(WireString::from_units(units.to_vec()))
            }
            Intrinsic::StringToLowerCase | Intrinsic::StringToUpperCase => {
                // Convert each maximal scalar sequence as a whole, preserving
                // contextual default casing and isolated surrogate units.
                let convert = |run: &str| {
                    if method == Intrinsic::StringToLowerCase { run.to_lowercase() } else { run.to_uppercase() }
                };
                let mut output = Vec::new();
                let mut run = String::new();
                for scalar in char::decode_utf16(text.units().iter().copied()) {
                    match scalar {
                        Ok(scalar) => run.push(scalar),
                        Err(error) => {
                            output.extend(convert(&run).encode_utf16());
                            run.clear();
                            output.push(error.unpaired_surrogate());
                        }
                    }
                }
                output.extend(convert(&run).encode_utf16());
                self.string(WireString::from_units(output))
            }
            Intrinsic::StringLastIndexOf => {
                // Source String.lastIndexOf converts the search argument before
                // position. Missing/NaN position selects +Infinity, unlike indexOf.
                let search = arguments.first().cloned().unwrap_or(self.undefined()?);
                let search = self.implicit_text(&search, runtime)?;
                let position =
                    arguments.get(1).map(|value| self.to_number(value, runtime)).transpose()?.unwrap_or(f64::NAN);
                let position = if position.is_nan() { f64::INFINITY } else { integer_or_infinity(position) };
                let start = position.clamp(0.0, text.len() as f64) as usize;
                let found = if search.is_empty() {
                    Some(start)
                } else if search.len() > text.len() {
                    None
                } else {
                    let last = start.min(text.len() - search.len());
                    (0..=last).rev().find(|&index| text.units()[index..index + search.len()] == *search.units())
                };
                self.number(found.map_or(-1.0, |index| index as f64))
            }
            Intrinsic::StringIncludes | Intrinsic::StringIndexOf => {
                let search = arguments.first().cloned().unwrap_or(self.undefined()?);
                if method == Intrinsic::StringIncludes && self.is_regexp(&search, runtime)? {
                    return Err(ValueError::TypeError("String.includes search is a RegExp".into()));
                }
                let search = self.implicit_text(&search, runtime)?;
                let position = arguments.get(1).map(|v| self.to_number(v, runtime)).transpose()?.unwrap_or(0.0);
                let start = integer_or_infinity(position).clamp(0.0, text.len() as f64) as usize;
                let found = find_units(text.units(), search.units(), start);
                if method == Intrinsic::StringIncludes {
                    self.boolean(found.is_some())
                } else {
                    self.number(found.map_or(-1.0, |index| index as f64))
                }
            }
            Intrinsic::StringCharCodeAt => {
                let position = arguments.first().map(|v| self.to_number(v, runtime)).transpose()?.unwrap_or(0.0);
                let index = integer_or_infinity(position);
                self.number(if index < 0.0 || index >= text.len() as f64 {
                    f64::NAN
                } else {
                    f64::from(text.units()[index as usize])
                })
            }
            Intrinsic::StringSplit => {
                let limit = match arguments.get(1) {
                    Some(limit) if self.kind(limit)? != ValueKind::Undefined => {
                        let n = self.to_number(limit, runtime)?;
                        if !n.is_finite() || n == 0.0 { 0 } else { n.trunc().rem_euclid(4_294_967_296.0) as u32 }
                    }
                    _ => u32::MAX,
                };
                let result = self.array_with_length(0)?;
                let separator = arguments.first().cloned().unwrap_or(self.undefined()?);
                if self.kind(&separator)? == ValueKind::Undefined {
                    if limit == 0 {
                        return Ok(result);
                    }
                    self.define_own_data(&result, "0".into(), self.string(text)?, runtime)?;
                    return Ok(result);
                }
                let separator = self.implicit_text(&separator, runtime)?;
                if limit == 0 {
                    return Ok(result);
                }
                let mut pieces = Vec::new();
                if separator.is_empty() {
                    pieces.extend(text.units().iter().take(limit as usize).map(|unit| vec![*unit]));
                } else {
                    let mut start = 0;
                    while let Some(index) = find_units(text.units(), separator.units(), start) {
                        pieces.push(text.units()[start..index].to_vec());
                        if pieces.len() == limit as usize {
                            break;
                        }
                        start = index + separator.len();
                    }
                    if pieces.len() < limit as usize {
                        pieces.push(text.units()[start..].to_vec());
                    }
                }
                for (index, piece) in pieces.into_iter().enumerate() {
                    self.define_own_data(
                        &result,
                        WireString::from(index.to_string()).into(),
                        self.string(WireString::from_units(piece))?,
                        runtime,
                    )?;
                }
                Ok(result)
            }
            Intrinsic::StringPadStart => {
                let length = arguments.first().map(|v| self.to_number(v, runtime)).transpose()?.unwrap_or(0.0);
                let length =
                    if length.is_nan() || length <= 0.0 { 0.0 } else { length.floor().min(9_007_199_254_740_991.0) };
                if length <= text.len() as f64 {
                    return self.string(text);
                }
                let fill = match arguments.get(1) {
                    Some(fill) if self.kind(fill)? != ValueKind::Undefined => self.implicit_text(fill, runtime)?,
                    _ => WireString::from(" "),
                };
                if fill.is_empty() {
                    return self.string(text);
                }
                if length > u32::MAX as f64 {
                    return Err(ValueError::RangeError("Invalid string length".into()));
                }
                let length = length as usize;
                let mut units = Vec::new();
                units.try_reserve_exact(length).map_err(|_| ValueError::Allocation)?;
                for index in 0..length - text.len() {
                    units.push(fill.units()[index % fill.len()]);
                }
                units.extend_from_slice(text.units());
                self.string(WireString::from_units(units))
            }
            _ => Err(ValueError::InvalidNode),
        }
    }
}

pub(super) fn find_units(text: &[u16], search: &[u16], start: usize) -> Option<usize> {
    if start > text.len() {
        return None;
    }
    if search.is_empty() {
        return Some(start);
    }
    text[start..].windows(search.len()).position(|value| value == search).map(|index| index + start)
}

impl ValueHeap {
    pub fn split_key(&self) -> PropertyKey {
        PropertyKey::Symbol(SymbolKey { heap_token: self.token, node: self.intrinsics.symbol_split })
    }
}
