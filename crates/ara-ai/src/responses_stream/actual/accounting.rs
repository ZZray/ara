//! Original-handle Responses usage accounting, fixed shared.ts 440-464 and
//! 3719-3802. Missing raw fields remain unknown under the ARA usage contract.
use crate::actual_provider::GraphBuilder;
use ara_rpc::value_heap::{ValueError, ValueHandle, ValueSnapshot};
type Result<T> = std::result::Result<T, ValueError>;
fn number(b: &GraphBuilder, root: &ValueHandle, key: &str) -> Result<Option<f64>> {
    if b.is_nullish(root)? {
        return Ok(None);
    }
    Ok(match b.0.heap().snapshot(&b.get(root, key)?)? {
        ValueSnapshot::Number(value) => Some(value),
        _ => None,
    })
}
fn set(b: &GraphBuilder, root: &ValueHandle, key: &str, value: Option<f64>) -> Result<()> {
    if let Some(value) = value {
        b.define(root, key, b.0.heap().number(value)?)?;
    }
    Ok(())
}
pub(super) fn populate(b: &GraphBuilder, output: &ValueHandle, raw: &ValueHandle) -> Result<()> {
    if b.is_nullish(raw)? {
        return Ok(());
    }
    let details = b.get(raw, "input_tokens_details")?;
    let out_details = b.get(raw, "output_tokens_details")?;
    let reported_input = number(b, raw, "input_tokens")?;
    let reported_output = number(b, raw, "output_tokens")?;
    let cached = match number(b, &details, "cached_tokens")? {
        Some(value) => Some(value),
        None => number(b, raw, "prompt_cache_hit_tokens")?,
    };
    let oi = number(b, &details, "orchestration_input_tokens")?.unwrap_or(0.0);
    let oc = number(b, &details, "orchestration_input_cached_tokens")?.unwrap_or(0.0);
    let oo = number(b, &out_details, "orchestration_output_tokens")?.unwrap_or(0.0);
    let total = number(b, raw, "total_tokens")?;
    let primary = reported_input.unwrap_or(0.0) + reported_output.unwrap_or(0.0);
    let separate = primary + oi + oo;
    let includes = total.is_some_and(|total| oi + oo > 0.0 && (total - primary).abs() <= (total - separate).abs());
    let orchestration_cached = oi.min(oc);
    let orchestration_input = (oi - orchestration_cached).max(0.0);
    let router_write = number(b, &details, "cache_write_tokens")?;
    let deepseek_write = number(b, raw, "prompt_cache_miss_tokens")?;
    let hit = b.get(raw, "prompt_cache_hit_tokens")?;
    let miss = b.get(raw, "prompt_cache_miss_tokens")?;
    let has_hit_miss = !matches!(b.0.heap().snapshot(&hit)?, ValueSnapshot::Undefined)
        && !matches!(b.0.heap().snapshot(&miss)?, ValueSnapshot::Undefined);
    let deepseek = has_hit_miss && router_write.is_none() && deepseek_write.is_some_and(|value| value > 0.0);
    let written = if deepseek { Some(0.0) } else { router_write.or(deepseek_write) };
    let cache = cached.map(|value| (value - if includes { orchestration_cached } else { 0.0 }).max(0.0));
    let input = reported_input
        .map(|value| (value - if includes { oi } else { 0.0 }).max(0.0))
        .map(|value| (value - cache.unwrap_or(0.0) - if deepseek { 0.0 } else { written.unwrap_or(0.0) }).max(0.0));
    let output_tokens = reported_output.map(|value| (value - if includes { oo } else { 0.0 }).max(0.0));
    let usage = b.object([])?;
    set(b, &usage, "input", input)?;
    set(b, &usage, "output", output_tokens)?;
    set(b, &usage, "cacheRead", cache)?;
    set(b, &usage, "cacheWrite", written)?;
    let orchestration_total = orchestration_input + orchestration_cached + oo;
    let computed = match (input, output_tokens) {
        (Some(i), Some(o)) => Some(i + o + cache.unwrap_or(0.0) + written.unwrap_or(0.0) + orchestration_total),
        _ => None,
    };
    set(b, &usage, "totalTokens", if orchestration_total > 0.0 { total.or(computed) } else { computed.or(total) })?;
    let reasoning = number(b, &out_details, "reasoning_tokens")?.filter(|value| *value > 0.0);
    set(b, &usage, "reasoningTokens", reasoning)?;
    if orchestration_total > 0.0 {
        let orchestration = b.object([])?;
        set(b, &orchestration, "input", (orchestration_input > 0.0).then_some(orchestration_input))?;
        set(b, &orchestration, "cacheRead", (orchestration_cached > 0.0).then_some(orchestration_cached))?;
        set(b, &orchestration, "output", (oo > 0.0).then_some(oo))?;
        b.define(&usage, "orchestration", orchestration)?;
    }
    let previous = b.get(output, "usage")?;
    let premium = b.get(&previous, "premiumRequests")?;
    if !matches!(b.0.heap().snapshot(&premium)?, ValueSnapshot::Undefined) {
        b.define(&usage, "premiumRequests", premium)?;
    }
    b.put(output, "usage", usage)
}
