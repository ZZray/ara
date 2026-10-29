//! Scoped Native numeric operations used by the fixed prompt formatter.
//! Exact mixed Number/BigInt comparison follows the existing ARA schema
//! engine (schema_validator/view.rs), without an ara-ai dependency cycle.
use super::{Exec, Result};
use ara_rpc::{
    WireString,
    value_heap::{BigIntDecimal, ValueError, ValueHandle, ValueKind, ValueSnapshot},
};
use std::cmp::Ordering;

pub(super) fn compare(ex: Exec<'_>, left: &ValueHandle, right: &ValueHandle, operator: &str) -> Result<bool> {
    // Both relational operands use number-hint ToPrimitive in evaluation
    // order. Keep repeated coercions at each source comparison site.
    let left = ex.heap.to_primitive(left, false, ex.runtime)?;
    let right = ex.heap.to_primitive(right, false, ex.runtime)?;
    let order = match (ex.heap.snapshot(&left)?, ex.heap.snapshot(&right)?) {
        (ValueSnapshot::String(left), ValueSnapshot::String(right)) => Some(left.units().cmp(right.units())),
        (ValueSnapshot::BigInt(left), ValueSnapshot::BigInt(right)) => {
            Some(compare_bigints(left.as_str(), right.as_str()))
        }
        (ValueSnapshot::BigInt(left), ValueSnapshot::String(right)) => {
            parse_string_bigint(&right).map(|right| compare_bigints(left.as_str(), &right))
        }
        (ValueSnapshot::String(left), ValueSnapshot::BigInt(right)) => {
            parse_string_bigint(&left).map(|left| compare_bigints(&left, right.as_str()))
        }
        (ValueSnapshot::BigInt(left), _) => {
            compare_number_bigint(ex.heap.to_number(&right, ex.runtime)?, left.as_str()).map(Ordering::reverse)
        }
        (_, ValueSnapshot::BigInt(right)) => {
            compare_number_bigint(ex.heap.to_number(&left, ex.runtime)?, right.as_str())
        }
        _ => ex.heap.to_number(&left, ex.runtime)?.partial_cmp(&ex.heap.to_number(&right, ex.runtime)?),
    };
    Ok(match operator {
        "<" => order == Some(Ordering::Less),
        ">" => order == Some(Ordering::Greater),
        "<=" => matches!(order, Some(Ordering::Less | Ordering::Equal)),
        ">=" => matches!(order, Some(Ordering::Greater | Ordering::Equal)),
        _ => false,
    })
}

fn numeric(ex: Exec<'_>, value: &ValueHandle) -> Result<ValueHandle> {
    let primitive = ex.heap.to_primitive(value, false, ex.runtime)?;
    if ex.heap.kind(&primitive)? == ValueKind::BigInt {
        Ok(primitive)
    } else {
        ex.heap.number(ex.heap.to_number(&primitive, ex.runtime)?)
    }
}
pub(super) fn subtract(ex: Exec<'_>, left: &ValueHandle, right: &ValueHandle) -> Result<ValueHandle> {
    // ToNumeric(left) completes before ToNumeric(right), including a left
    // Symbol error. Both actual conversions precede a mixed-kind error.
    let left = numeric(ex, left)?;
    let right = numeric(ex, right)?;
    match (ex.heap.snapshot(&left)?, ex.heap.snapshot(&right)?) {
        (ValueSnapshot::BigInt(_), ValueSnapshot::BigInt(right)) => {
            let negative = match right.as_str().strip_prefix('-') {
                Some(positive) => positive.to_owned(),
                None if right.as_str() == "0" => "0".into(),
                None => format!("-{}", right.as_str()),
            };
            ex.heap.add(&left, &ex.heap.bigint_decimal(&negative)?, ex.runtime)
        }
        (ValueSnapshot::Number(left), ValueSnapshot::Number(right)) => ex.heap.number(left - right),
        _ => Err(ValueError::TypeError("Cannot mix BigInt and other types".into())),
    }
}

fn compare_bigints(left: &str, right: &str) -> Ordering {
    let left_negative = left.starts_with('-');
    let right_negative = right.starts_with('-');
    if left_negative != right_negative {
        return if left_negative { Ordering::Less } else { Ordering::Greater };
    }
    let left = left.trim_start_matches('-');
    let right = right.trim_start_matches('-');
    let order = left.len().cmp(&right.len()).then_with(|| left.cmp(right));
    if left_negative { order.reverse() } else { order }
}
fn multiply_decimal(digits: &mut Vec<u8>, multiplier: u16, mut carry: u16) {
    for digit in digits.iter_mut() {
        let value = u16::from(*digit) * multiplier + carry;
        *digit = (value % 10) as u8;
        carry = value / 10;
    }
    while carry != 0 {
        digits.push((carry % 10) as u8);
        carry /= 10;
    }
}
fn compare_number_bigint(number: f64, bigint: &str) -> Option<Ordering> {
    if number.is_nan() {
        return None;
    }
    if number == f64::INFINITY {
        return Some(Ordering::Greater);
    }
    if number == f64::NEG_INFINITY {
        return Some(Ordering::Less);
    }
    let bits = number.abs().to_bits();
    let field = ((bits >> 52) & 0x7ff) as i32;
    let mantissa = (bits & ((1_u64 << 52) - 1)) | if field == 0 { 0 } else { 1_u64 << 52 };
    let shift = if field == 0 { -1022 - 52 } else { field - 1023 - 52 };
    let mut integer = if shift < 0 { if -shift >= 64 { 0 } else { mantissa >> (-shift) } } else { mantissa };
    let mut digits = Vec::new();
    loop {
        digits.push((integer % 10) as u8);
        integer /= 10;
        if integer == 0 {
            break;
        }
    }
    for _ in 0..shift.max(0) {
        multiply_decimal(&mut digits, 2, 0);
    }
    let mut text: String = digits.iter().rev().map(|digit| char::from(b'0' + digit)).collect();
    if number.is_sign_negative() && text != "0" {
        text.insert(0, '-');
    }
    let order = compare_bigints(&text, bigint);
    Some(if order == Ordering::Equal && number.fract() != 0.0 {
        if number.is_sign_negative() { Ordering::Less } else { Ordering::Greater }
    } else {
        order
    })
}
fn parse_string_bigint(value: &WireString) -> Option<String> {
    let text = value.to_utf8().ok()?;
    let text = text.trim_matches(|ch: char| matches!(ch, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'));
    if text.is_empty() {
        return Some("0".into());
    }
    if let Ok(value) = BigIntDecimal::parse(text) {
        return Some(value.as_str().into());
    }
    let (radix, digits) = if let Some(digits) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        (16, digits)
    } else if let Some(digits) = text.strip_prefix("0o").or_else(|| text.strip_prefix("0O")) {
        (8, digits)
    } else {
        (2, text.strip_prefix("0b").or_else(|| text.strip_prefix("0B"))?)
    };
    if digits.is_empty() {
        return None;
    }
    let mut decimal = vec![0];
    for ch in digits.chars() {
        multiply_decimal(&mut decimal, radix as u16, ch.to_digit(radix)? as u16);
    }
    Some(decimal.iter().rev().map(|digit| char::from(b'0' + digit)).collect())
}
