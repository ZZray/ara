//! Fixed OMP 596f2da profiler helpers. MIT; Copyright (c) 2025 Mario Zechner;
//! 2025-2026 Can Bölük; 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md.
//! Pure parsers/renderers; all rendered text retains UTF-16 code units.

mod cpu;
mod sample;
mod tree;

use ara_rpc::json::{WireString, WireValue};
pub use cpu::{
    CpuProfile, CpuProfileNode, is_cpu_profile_path, parse_cpu_profile, render_cpu_profile,
};
pub use sample::{
    SampleFrame, SampleHeader, SampleProfile, SampleThread, demangle_symbol,
    is_sample_profile_path, parse_sample_profile, render_sample_profile,
};
pub use tree::{
    ProfileNode, RenderTreeContext, flatten_recursion, format_pct, merge_into, render_profile_node,
};

#[derive(Clone, Debug, PartialEq)]
pub struct ProfileError {
    pub kind: &'static str,
    pub message: WireString,
}
impl ProfileError {
    fn type_error(message: &str) -> Self {
        Self {
            kind: "TypeError",
            message: message.into(),
        }
    }
}
impl std::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: {}",
            self.kind,
            String::from_utf16_lossy(self.message.units())
        )
    }
}
impl std::error::Error for ProfileError {}

pub(crate) fn cat(parts: &[&WireString]) -> WireString {
    WireString::from_units(
        parts
            .iter()
            .flat_map(|part| part.units().iter().copied())
            .collect(),
    )
}
pub(crate) fn append(text: &mut WireString, other: &WireString) {
    *text = cat(&[text, other]);
}
pub(crate) fn join(parts: &[WireString], separator: &str) -> WireString {
    let separator: WireString = separator.into();
    let mut out = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            out.extend_from_slice(separator.units());
        }
        out.extend_from_slice(part.units());
    }
    WireString::from_units(out)
}
pub(crate) fn pad(text: &WireString, width: usize) -> WireString {
    let mut out = vec![32; width.saturating_sub(text.len())];
    out.extend_from_slice(text.units());
    WireString::from_units(out)
}
pub(crate) fn number(value: f64) -> WireString {
    ara_prompt::js::f64_to_string(value).into()
}
pub(crate) fn max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a == b && a == 0.0 {
        if a.is_sign_positive() || b.is_sign_positive() {
            0.0
        } else {
            -0.0
        }
    } else {
        a.max(b)
    }
}
pub(crate) fn round(value: f64) -> f64 {
    if !value.is_finite() || value == 0.0 || value.abs() >= (1_u64 << 52) as f64 {
        return value;
    }
    let floor = value.floor();
    let result = if value - floor < 0.5 {
        floor
    } else {
        floor + 1.0
    };
    if result == 0.0 && value < 0.0 {
        -0.0
    } else {
        result
    }
}
/// Number.toFixed(0..2): exact binary64 rational, larger decimal integer on tie.
/// The profile renderers only request one or two digits. Never Rust ties-to-even.
pub(crate) fn fixed(value: f64, digits: u32) -> WireString {
    debug_assert!(digits <= 2);
    if !value.is_finite() || value.abs() >= 1e21 {
        return number(value);
    }
    let negative = value < 0.0;
    let bits = value.abs().to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i32;
    let mantissa =
        u128::from(bits & ((1_u64 << 52) - 1)) | if exponent == 0 { 0 } else { 1_u128 << 52 };
    let shift = if exponent == 0 { 1074 } else { 1075 - exponent };
    let scale = 10_u128.pow(digits);
    let scaled = mantissa * scale;
    let integer = if shift >= 128 {
        0
    } else if shift > 0 {
        let denominator = 1_u128 << shift;
        (scaled + denominator / 2) / denominator
    } else {
        scaled << (-shift)
    };
    let sign = if negative { "-" } else { "" };
    if digits == 0 {
        format!("{sign}{integer}").into()
    } else {
        format!(
            "{sign}{}.{:0width$}",
            integer / scale,
            integer % scale,
            width = digits as usize
        )
        .into()
    }
}
pub(crate) fn truthy(value: Option<&WireValue>) -> bool {
    match value {
        None | Some(WireValue::Null) => false,
        Some(WireValue::Bool(value)) => *value,
        Some(WireValue::Number(value)) => *value != 0.0 && !value.is_nan(),
        Some(WireValue::String(value)) => !value.is_empty(),
        Some(WireValue::Array(_) | WireValue::Object(_)) => true,
    }
}
pub(crate) fn primitive(value: &WireValue) -> Result<WireValue, ProfileError> {
    match value {
        WireValue::Array(values) => {
            let mut parts = Vec::new();
            for value in values {
                parts.push(if matches!(value, WireValue::Null) {
                    "".into()
                } else {
                    js_string(Some(value))?
                });
            }
            Ok(WireValue::String(join(&parts, ",")))
        }
        WireValue::Object(_) => {
            // JSON cannot contain callable methods. A shadowed toString is not
            // callable; inherited valueOf returns the object, so conversion throws.
            if value.get("toString").is_some() {
                return Err(ProfileError::type_error(
                    "Cannot convert object to primitive value",
                ));
            }
            Ok(WireValue::String("[object Object]".into()))
        }
        _ => Ok(value.clone()),
    }
}
pub(crate) fn js_string(value: Option<&WireValue>) -> Result<WireString, ProfileError> {
    Ok(match value {
        None => "undefined".into(),
        Some(WireValue::Null) => "null".into(),
        Some(WireValue::Bool(value)) => value.to_string().into(),
        Some(WireValue::Number(value)) => number(*value),
        Some(WireValue::String(value)) => value.clone(),
        Some(value) => match &primitive(value)? {
            WireValue::String(value) => value.clone(),
            _ => unreachable!(),
        },
    })
}
pub(crate) fn to_number(value: Option<&WireValue>) -> Result<f64, ProfileError> {
    match value {
        None => Ok(f64::NAN),
        Some(WireValue::Null) => Ok(0.0),
        Some(WireValue::Bool(value)) => Ok(if *value { 1.0 } else { 0.0 }),
        Some(WireValue::Number(value)) => Ok(*value),
        Some(WireValue::String(value)) => {
            let units = trim(value.units());
            if units.is_empty() {
                return Ok(0.0);
            }
            let Ok(text) = String::from_utf16(units) else {
                return Ok(f64::NAN);
            };
            if text == "Infinity" || text == "+Infinity" {
                return Ok(f64::INFINITY);
            }
            if text == "-Infinity" {
                return Ok(f64::NEG_INFINITY);
            }
            for (prefix, radix) in [
                ("0x", 16),
                ("0X", 16),
                ("0b", 2),
                ("0B", 2),
                ("0o", 8),
                ("0O", 8),
            ] {
                if let Some(body) = text.strip_prefix(prefix) {
                    if body.is_empty() {
                        return Ok(f64::NAN);
                    }
                    let mut result = 0.0;
                    for ch in body.chars() {
                        let Some(digit) = ch.to_digit(radix) else {
                            return Ok(f64::NAN);
                        };
                        result = result * f64::from(radix) + f64::from(digit);
                    }
                    return Ok(result);
                }
            }
            // Rust accepts "inf"; ECMAScript StringNumericLiteral does not.
            if text
                .bytes()
                .any(|ch| !(ch.is_ascii_digit() || matches!(ch, b'+' | b'-' | b'.' | b'e' | b'E')))
            {
                return Ok(f64::NAN);
            }
            Ok(text.parse::<f64>().unwrap_or(f64::NAN))
        }
        Some(value) => to_number(Some(&primitive(value)?)),
    }
}
pub(crate) fn add(left: &WireValue, right: &WireValue) -> Result<WireValue, ProfileError> {
    let left = primitive(left)?;
    let right = primitive(right)?;
    if matches!(left, WireValue::String(_)) || matches!(right, WireValue::String(_)) {
        Ok(WireValue::String(cat(&[
            &js_string(Some(&left))?,
            &js_string(Some(&right))?,
        ])))
    } else {
        Ok(WireValue::Number(
            to_number(Some(&left))? + to_number(Some(&right))?,
        ))
    }
}
pub(crate) fn space(unit: u16) -> bool {
    char::from_u32(u32::from(unit)).is_some_and(ara_prompt::js::is_space)
}
pub(crate) fn trim(mut units: &[u16]) -> &[u16] {
    while units.first().is_some_and(|unit| space(*unit)) {
        units = &units[1..];
    }
    while units.last().is_some_and(|unit| space(*unit)) {
        units = &units[..units.len() - 1];
    }
    units
}
pub(crate) fn ascii_suffix(file: &str, suffix: &str) -> bool {
    let units: Vec<u16> = file.encode_utf16().collect();
    let end = units.len();
    // The source has no multiline flag: ECMAScript $ asserts absolute end.
    end >= suffix.len()
        && units[end - suffix.len()..end]
            .iter()
            .zip(suffix.bytes())
            .all(|(&unit, ch)| unit <= 127 && (unit as u8).eq_ignore_ascii_case(&ch))
}
