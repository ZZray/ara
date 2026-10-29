//! Fixed OMP 596f2da, packages/ai/src/dialect/coercion.ts (MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See LICENSE.

use crate::relaxed_json::{FinalParseError, FinalValueFactory, parse_json_with_repair_units};
use ara_rpc::WireString;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
static TOOL_CALL_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Native asRecord(value): allocation occurs only for a non-record value.
pub(super) fn as_record_value<F: FinalValueFactory>(factory: &F, value: F::Value) -> Result<F::Value, F::Error> {
    if factory.is_record(&value)? { Ok(value) } else { factory.object() }
}

/// Same source body in Hermes:144–159 and Qwen3:177–192. This helper is
/// source reuse, not a normalization of their distinct early-start policies.
#[allow(clippy::type_complexity, reason = "Native name and arguments retain their actual value carriers.")]
pub(super) fn parse_named_call<F: FinalValueFactory>(
    body: &WireString,
    factory: &F,
) -> Result<Option<(F::Value, F::Value)>, F::Error> {
    let attempt = (|| {
        let parsed = parse_json_with_repair_units(&super::units::wire(super::units::trim(body.units())), factory)?;
        let first_name = factory.get(&parsed, &"name".into()).map_err(FinalParseError::Runtime)?;
        if factory.string_value(&first_name).map_err(FinalParseError::Runtime)?.is_none() {
            return Ok(None);
        }
        let second_name = factory.get(&parsed, &"name".into()).map_err(FinalParseError::Runtime)?;
        let length = factory.get(&second_name, &"length".into()).map_err(FinalParseError::Runtime)?;
        if factory.is_number_zero(&length).map_err(FinalParseError::Runtime)? {
            return Ok(None);
        }
        let mut args = factory.get(&parsed, &"arguments".into()).map_err(FinalParseError::Runtime)?;
        if let Some(text) = factory.string_value(&args).map_err(FinalParseError::Runtime)? {
            args = match parse_json_with_repair_units(&text, factory) {
                Ok(value) => value,
                Err(FinalParseError::Syntax(_)) => factory.object().map_err(FinalParseError::Runtime)?,
                Err(FinalParseError::Runtime(error)) if factory.should_catch_runtime_error(&error) => {
                    factory.object().map_err(FinalParseError::Runtime)?
                }
                Err(error) => return Err(error),
            };
        }
        // Return literal evaluates name before asRecord(arguments), after all
        // args repair effects. This third value is not revalidated/coerced.
        let name = factory.get(&parsed, &"name".into()).map_err(FinalParseError::Runtime)?;
        let args = as_record_value(factory, args).map_err(FinalParseError::Runtime)?;
        Ok(Some((name, args)))
    })();
    match attempt {
        Ok(value) => Ok(value),
        Err(FinalParseError::Syntax(_)) => Ok(None),
        Err(FinalParseError::Runtime(error)) if factory.should_catch_runtime_error(&error) => Ok(None),
        Err(FinalParseError::Runtime(error)) => Err(error),
    }
}

fn base36(mut value: u128) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if value == 0 {
        return "0".into();
    }
    let mut out = Vec::new();
    while value != 0 {
        out.push(DIGITS[(value % 36) as usize]);
        value /= 36;
    }
    out.reverse();
    String::from_utf8(out).expect("base36 is ASCII")
}

pub fn mint_tool_call_id() -> String {
    let previous = TOOL_CALL_COUNTER
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |counter| Some((counter + 1) % MAX_SAFE_INTEGER))
        .expect("counter update always succeeds");
    let counter = (previous + 1) % MAX_SAFE_INTEGER;
    let millis = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => base36(duration.as_millis()),
        Err(error) => format!("-{}", base36(error.duration().as_millis())),
    };
    format!("ptc_{millis}_{}", base36(u128::from(counter)))
}

/// Prefix lengths are UTF-8 offsets for the Rust stream buffer. Fixed Native
/// control tags are ASCII, so those offsets equal its UTF-16 offsets.
pub fn partial_suffix_overlap(text: &str, tag: &str) -> usize {
    let maximum = text.len().min(tag.len().saturating_sub(1));
    (1..=maximum).rev().find(|&length| tag.is_char_boundary(length) && text.ends_with(&tag[..length])).unwrap_or(0)
}

pub fn partial_suffix_overlap_any(text: &str, tags: &[&str]) -> usize {
    tags.iter().map(|tag| partial_suffix_overlap(text, tag)).max().unwrap_or(0)
}

pub fn normalize_kimi_function_name(raw_id: &str) -> String {
    let before_index = raw_id.split(':').next().unwrap_or(raw_id);
    ara_prompt::js::trim(before_index.rsplit('.').next().unwrap_or(before_index)).into()
}
