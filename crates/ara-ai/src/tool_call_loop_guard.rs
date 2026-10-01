//! Consecutive tool-call batches from fixed OMP `packages/ai/src/utils/tool-call-loop-guard.ts`.
//!
//! Source: 596f2da7101178214aa27a753529d15e6b7ad91d. Upstream portions are MIT;
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See `THIRD_PARTY_NOTICES.md`.
//!
//! This detector owns no tools, host policy, settings, retry, or Session state.
//! The hash is the native JSON string of the canonical tool-call batch, not the
//! tool results. Summaries retain UTF-16 code units because the upstream's slice
//! can end between a surrogate pair; conversion to Rust UTF-8 is checked.

use crate::{AssistantMessage, ToolCall, ToolResultMessage, UserBlock};
use ara_rpc::json::{WireString, WireValue};
use std::collections::HashSet;

// Fixed packages/wire/src/index.ts:400, not a guessed harness field name.
const INTENT_FIELD: &str = "i";
const LEGACY_INTENT_FIELD: &str = "__intent";
const RESULT_SUMMARY_LIMIT: usize = 200;
const ARGUMENT_SUMMARY_LIMIT: usize = 400;

/// Runtime settings for cross-turn tool-call repetition detection.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolCallLoopGuardOptions {
    pub threshold: f64,
    pub exempt_tools: Vec<String>,
}

/// Details needed to steer the model away from a repeated tool call.
///
/// Use [`Self::to_wire_value`] for lossless native JSON, including a summary
/// ending in an unpaired surrogate. [`WireString::to_utf8`] explicitly reports
/// summaries that cannot be represented by a Rust `String`.
#[derive(Clone, Debug, PartialEq)]
pub struct RepeatedToolCallDetection {
    pub kind: &'static str,
    pub tool_name: String,
    pub count: f64,
    pub result_summary: WireString,
    pub arguments_summary: WireString,
}

impl RepeatedToolCallDetection {
    /// Native field names and serialization without replacing UTF-16 units.
    pub fn to_wire_value(&self) -> WireValue {
        WireValue::object(vec![
            ("kind", WireValue::String(self.kind.into())),
            ("toolName", WireValue::String(self.tool_name.as_str().into())),
            ("count", WireValue::Number(self.count)),
            ("resultSummary", WireValue::String(self.result_summary.clone())),
            ("argumentsSummary", WireValue::String(self.arguments_summary.clone())),
        ])
    }
}

/// Detects consecutive identical assistant tool calls across model turns.
#[derive(Debug)]
pub struct ToolCallLoopGuard {
    threshold: f64,
    exempt_tools: HashSet<String>,
    last_hash: Option<String>,
    count: f64,
}

impl ToolCallLoopGuard {
    pub fn new(options: ToolCallLoopGuardOptions) -> Self {
        // Rust f64::max chooses the other operand for NaN; Math.max preserves
        // NaN. Infinity and truncation also retain the upstream Number semantics.
        let threshold = if options.threshold.is_nan() { f64::NAN } else { options.threshold.trunc().max(1.0) };
        Self { threshold, exempt_tools: options.exempt_tools.into_iter().collect(), last_hash: None, count: 0.0 }
    }

    /// Records one completed turn and returns only the exact threshold hit.
    pub fn record_turn(
        &mut self,
        message: &AssistantMessage,
        tool_results: &[ToolResultMessage],
    ) -> Option<RepeatedToolCallDetection> {
        let tool_calls: Vec<&ToolCall> = message.content.iter().filter_map(|part| part.as_tool_call()).collect();
        if tool_calls.is_empty() || tool_calls.iter().all(|call| self.exempt_tools.contains(&call.name)) {
            self.last_hash = None;
            self.count = 0.0;
            return None;
        }

        // Exempt members still participate in a mixed batch's identity. Sorting
        // strings uses JS's UTF-16 order, not Rust's UTF-8/scalar ordering.
        let mut canonical_calls: Vec<String> = tool_calls
            .iter()
            .map(|call| {
                WireValue::Array(vec![WireValue::String(call.name.as_str().into()), canonical_arguments(call)])
                    .stringify()
            })
            .collect();
        canonical_calls.sort_by(|left, right| left.encode_utf16().cmp(right.encode_utf16()));
        let turn_hash = WireValue::Array(
            canonical_calls.into_iter().map(|call| WireValue::String(WireString::from(call))).collect(),
        )
        .stringify();
        if self.last_hash.as_ref() == Some(&turn_hash) {
            self.count += 1.0;
        } else {
            self.last_hash = Some(turn_hash);
            self.count = 1.0;
        }

        if self.count != self.threshold {
            return None;
        }
        let report_call = tool_calls
            .iter()
            .find(|call| !self.exempt_tools.contains(&call.name))
            .expect("a non-exempt call remains after the all-exempt reset");
        Some(RepeatedToolCallDetection {
            kind: "repeated_tool_call",
            tool_name: report_call.name.clone(),
            count: self.count,
            result_summary: summarize_tool_result(tool_results, &report_call.id),
            arguments_summary: summarize_text(&canonical_arguments(report_call).stringify(), ARGUMENT_SUMMARY_LIMIT),
        })
    }
}

fn canonical_arguments(call: &ToolCall) -> WireValue {
    // Reuse the native JSON representation already used by RPC. Parsing the
    // Rust JSON spelling as Number/f64 applies the same numeric rounding as JS
    // JSON.parse, and stringify provides ECMAScript exponent and -0 formatting.
    let json = serde_json::to_string(&call.arguments).expect("JSON tool arguments serialize");
    let native = WireValue::parse(&json).expect("serialized JSON tool arguments parse");
    canonicalize_tool_call_value(&native)
}

fn canonicalize_tool_call_value(value: &WireValue) -> WireValue {
    match value {
        WireValue::Array(items) => WireValue::Array(items.iter().map(canonicalize_tool_call_value).collect()),
        WireValue::Object(entries) => {
            let mut ordered: Vec<_> = entries.iter().collect();
            ordered.sort_by(|(left, _), (right, _)| left.units().cmp(right.units()));
            WireValue::Object(
                ordered
                    .into_iter()
                    .filter(|(key, _)| {
                        !key.equals_ascii(INTENT_FIELD)
                            && !key.equals_ascii(LEGACY_INTENT_FIELD)
                            // Upstream assigns to {}. Its inherited __proto__
                            // setter never creates this as an own property.
                            && !key.equals_ascii("__proto__")
                    })
                    .map(|(key, item)| (key.clone(), canonicalize_tool_call_value(item)))
                    .collect(),
            )
        }
        _ => value.clone(),
    }
}

fn summarize_tool_result(tool_results: &[ToolResultMessage], tool_call_id: &str) -> WireString {
    let Some(result) = tool_results.iter().find(|result| result.tool_call_id == tool_call_id) else {
        return WireString::from("");
    };
    let text = result
        .content
        .iter()
        .filter_map(|part| match part {
            UserBlock::Text(text) => Some(text.text.as_str()),
            UserBlock::Image(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    summarize_text(&text, RESULT_SUMMARY_LIMIT)
}

fn summarize_text(text: &str, limit: usize) -> WireString {
    // Same JS whitespace set as thinking_loop; Rust Unicode whitespace differs
    // at U+0085 and U+FEFF. Normalize/trim before applying the UTF-16 slice.
    let mut units = Vec::new();
    let mut pending_space = false;
    for unit in text.encode_utf16() {
        if matches!(unit, 0x0009..=0x000d | 0x0020 | 0x00a0 | 0x1680 | 0x2000..=0x200a | 0x2028 | 0x2029 | 0x202f | 0x205f | 0x3000 | 0xfeff)
        {
            pending_space = !units.is_empty();
        } else {
            if pending_space {
                units.push(0x0020);
                pending_space = false;
            }
            units.push(unit);
        }
    }
    if units.len() > limit {
        units.truncate(limit);
        units.push(0x2026);
    }
    WireString::from_units(units)
}
