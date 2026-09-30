//! Lossless Host policy/build boundary for fixed OMP 596f2da.
//!
//! Taxonomy/cascade string operations and constructor/correction arithmetic
//! run on UTF-16/f64. The existing policy detector consumes its reviewed ASCII
//! facts and axes; authored open records retain their native wire values.
//
// MIT License
// Copyright (c) 2025 Mario Zechner
// Copyright (c) 2025-2026 Can Bölük
// Copyright (c) 2026 Stencil Labs, Inc.
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use ara_rpc::{WireString, WireValue};
use serde_json::{Value, json};

use crate::{
    catalog_rules,
    js_regex::JsRegExp,
    model_collapse::{CollapseError, CollapseModelPolicy, VariantSpec, lower, strip_thinking_variant, trim, truthy},
    model_policy,
};

pub struct WireModelPolicy;

fn object() -> VariantSpec {
    VariantSpec::from_wire(WireValue::Object(Vec::new()))
}
fn s(value: &str) -> WireValue {
    WireValue::String(value.into())
}
fn get_string(spec: &VariantSpec, key: &str) -> WireString {
    spec.get(key).and_then(WireValue::as_string).cloned().unwrap_or_else(|| WireString::from(""))
}
fn js_string(value: &Value) -> &str {
    value.as_str().expect("compiled string")
}
fn array(value: &Value) -> &[Value] {
    value.as_array().expect("compiled array")
}
fn equal_ascii(value: &WireString, expected: &str) -> bool {
    value.equals_ascii(expected)
}
fn slice(value: &WireString, start: usize, end: usize) -> WireString {
    let start = start.min(value.len());
    WireString::from_units(value.units()[start..end.min(value.len()).max(start)].to_vec())
}
fn bare(value: &WireString) -> WireString {
    slice(value, value.units().iter().rposition(|u| *u == b'/' as u16).map_or(0, |i| i + 1), value.len())
}
fn starts(value: &WireString, expected: &str) -> bool {
    value.units().starts_with(&expected.encode_utf16().collect::<Vec<_>>())
}
fn ends(value: &WireString, expected: &str) -> bool {
    value.units().ends_with(&expected.encode_utf16().collect::<Vec<_>>())
}
fn find(value: &[u16], needle: &[u16]) -> Option<usize> {
    if needle.is_empty() { Some(0) } else { value.windows(needle.len()).position(|window| window == needle) }
}
fn contains(values: &Value, value: &WireString) -> bool {
    array(values).iter().any(|candidate| equal_ascii(value, js_string(candidate)))
}
fn bounded(value: &WireString, token: &str) -> bool {
    if equal_ascii(value, token) {
        return true;
    }
    starts(value, token)
        && value.units().get(token.encode_utf16().count()).is_some_and(|u| matches!(*u, 45 | 95 | 46 | 58 | 48..=57))
}
/// cascade.ts uses anchored literal segments, not a Unicode regexp.
fn glob(pattern: &str, value: &WireString) -> bool {
    let segments = pattern.split('*').map(|part| part.encode_utf16().collect::<Vec<_>>()).collect::<Vec<_>>();
    if segments.len() == 1 {
        return value.units() == segments[0];
    }
    if !value.units().starts_with(&segments[0]) {
        return false;
    }
    let mut remaining = &value.units()[segments[0].len()..];
    for segment in &segments[1..segments.len() - 1] {
        if segment.is_empty() {
            continue;
        }
        let Some(index) = find(remaining, segment) else {
            return false;
        };
        remaining = &remaining[index + segment.len()..];
    }
    remaining.ends_with(segments.last().unwrap())
}
fn matcher(m: &Value, normalized: &WireString, model_bare: &WireString) -> bool {
    let token = js_string(&m["token"]);
    match js_string(&m["kind"]) {
        "exact" => equal_ascii(model_bare, token),
        "bounded" => bounded(model_bare, token),
        "namespace" => normalized
            .units()
            .split(|u| *u == 47 || (m["bounded"] == true && matches!(*u, 46 | 58)))
            .filter(|part| !part.is_empty())
            .any(|part| {
                let part = WireString::from_units(part.to_vec());
                if m["bounded"] == true { bounded(&part, token) } else { equal_ascii(&part, token) }
            }),
        "prefix" => starts(model_bare, token),
        "glob" => glob(token, model_bare),
        _ => false,
    }
}
type FamilyRank<'a> = (Option<((f64, usize), &'a str)>, Option<(&'a str, &'a str)>);
fn rank_families<'a>(class: &'a Value, subject: &WireString) -> FamilyRank<'a> {
    let mut winner: Option<((f64, usize), &str)> = None;
    let mut tied = None;
    for family in array(&class["families"]) {
        let pattern = js_string(&family["glob"]);
        if !glob(pattern, subject) {
            continue;
        }
        let rank = (family["priority"].as_f64().unwrap(), pattern.encode_utf16().filter(|u| *u != 42).count());
        let id = js_string(&family["id"]);
        if let Some((held, held_id)) = winner {
            if held == rank && held_id != id {
                tied = Some((held_id, id));
            } else if held < rank {
                winner = Some((rank, id));
                tied = None;
            }
        } else {
            winner = Some((rank, id));
        }
    }
    (winner, tied)
}
fn ambiguity(model: &WireString, first: &str, second: &str, kind: &str) -> CollapseError {
    let mut units = format!("ambiguous {kind} for `").encode_utf16().collect::<Vec<_>>();
    units.extend(model.units());
    units.extend(format!("`: `{first}` and `{second}` tie").encode_utf16());
    CollapseError::from_wire(WireString::from_units(units))
}
fn family(
    class: &Value,
    model_bare: &WireString,
    model: &WireString,
    lenient: bool,
) -> Result<Option<String>, CollapseError> {
    let (winner, tied) = rank_families(class, model_bare);
    let Some((first, second)) = tied else {
        return Ok(winner.map(|(_, id)| id.to_owned()));
    };
    if let Some(separator) = model_bare.units().iter().position(|u| matches!(*u, 46 | 58))
        && separator > 0
        && separator + 1 < model_bare.len()
        && array(&class["matchers"])
            .iter()
            .any(|m| equal_ascii(&slice(model_bare, 0, separator), js_string(&m["token"])))
    {
        let (rescored, tied) = rank_families(class, &slice(model_bare, separator + 1, model_bare.len()));
        if tied.is_none() && rescored.is_some() {
            return Ok(rescored.map(|(_, id)| id.to_owned()));
        }
    }
    if lenient { Ok(None) } else { Err(ambiguity(model, first, second, "family")) }
}
fn revision(class: &Value, model_bare: &WireString) -> Option<String> {
    if contains(&class["skipBare"], model_bare) {
        return None;
    }
    for rule in array(&class["revisionPrefixes"]) {
        let prefix = js_string(&rule["prefix"]).encode_utf16().collect::<Vec<_>>();
        let start = if rule["anywhere"] == true {
            find(model_bare.units(), &prefix)
        } else if model_bare.units().starts_with(&prefix) {
            Some(0)
        } else {
            None
        };
        let Some(start) = start else {
            continue;
        };
        let tail = &model_bare.units()[start + prefix.len()..];
        let digit = tail.iter().position(|u| matches!(*u, 48..=57))?;
        return revision_prefix(&tail[digit..]).map(catalog_rules::format_revision);
    }
    None
}
fn revision_prefix(value: &[u16]) -> Option<catalog_rules::Revision> {
    let mut out = [0; 3];
    let mut index = 0;
    for count in 0..3 {
        let start = index;
        while value.get(index).is_some_and(|u| matches!(*u, 48..=57)) {
            index += 1;
        }
        let component = if value.get(index).is_some_and(|u| matches!(*u,65..=90|97..=122)) {
            None
        } else {
            String::from_utf16(&value[start..index]).ok().and_then(|s| s.parse::<u8>().ok())
        };
        let Some(component) = component else {
            return (count > 0).then_some(out);
        };
        out[count] = component;
        if !value.get(index).is_some_and(|u| matches!(*u, 45 | 46))
            || !value.get(index + 1).is_some_and(|u| matches!(*u, 48..=57))
        {
            break;
        }
        index += 1;
    }
    Some(out)
}
fn ranks(model: &WireString, explicit_class: Option<&str>, lenient: bool) -> Result<VariantSpec, CollapseError> {
    let classes = array(&catalog_rules::compiled_rules()["taxonomy"]["classes"]);
    let normalized = lower(&trim(model));
    let model_bare = bare(&normalized);
    let class = if let Some(id) = explicit_class {
        classes.iter().find(|c| c["id"] == id)
    } else {
        let mut winner: Option<((u8, usize), &Value)> = None;
        let mut tied = None;
        for class in classes {
            for m in array(&class["matchers"]) {
                if !matcher(m, &normalized, &model_bare) {
                    continue;
                }
                let rank = (
                    match js_string(&m["kind"]) {
                        "exact" => 4,
                        "bounded" => 3,
                        "namespace" => 2,
                        "prefix" => 1,
                        _ => 0,
                    },
                    js_string(&m["token"]).encode_utf16().count(),
                );
                if let Some((held, held_class)) = winner {
                    if held == rank && held_class["id"] != class["id"] {
                        tied = Some((js_string(&held_class["id"]), js_string(&class["id"])));
                    } else if held < rank {
                        winner = Some((rank, class));
                        tied = None;
                    }
                } else {
                    winner = Some((rank, class));
                }
            }
        }
        if let Some((first, second)) = tied {
            if !lenient {
                return Err(ambiguity(&normalized, first, second, "class"));
            }
            None
        } else {
            winner.map(|(_, class)| class)
        }
    };
    let mut out = object();
    if explicit_class.is_none() {
        out.set("class", class.map_or_else(|| s("unknown"), |c| VariantSpec::from_json(&c["id"]).value));
    }
    if let Some(class) = class {
        if let Some(family) = family(class, &model_bare, &normalized, lenient)? {
            out.set("family", s(&family));
        }
        if let Some(revision) = revision(class, &model_bare) {
            out.set("revision", s(&revision));
        }
    }
    Ok(out)
}
fn collapsed_id(provider: &WireString, model: &WireString) -> VariantSpec {
    let vocabulary = catalog_rules::collapse_vocabulary();
    let normalized = lower(model);
    let normalized_provider = lower(provider);
    let mut out = object();
    let mut logical = model.clone();
    let mut thinking = false;
    for family in array(&vocabulary["effortFamilies"]) {
        if equal_ascii(&normalized_provider, js_string(&family["provider"]))
            && contains(&family["aliases"], &normalized)
        {
            logical = js_string(&family["logical"]).into();
            out.set("logicalId", WireValue::String(logical));
            out.set("thinkingVariant", WireValue::Bool(false));
            return out;
        }
    }
    let normalized_bare = bare(&normalized);
    let mut winner: Option<&Value> = None;
    for rule in array(&vocabulary["suffixes"]) {
        let suffix = js_string(&rule["suffix"]);
        if !ends(&normalized, suffix)
            || rule.get("exceptBarePrefix").is_some_and(|p| starts(&normalized_bare, js_string(p)))
        {
            continue;
        }
        if winner.is_none_or(|held| suffix.encode_utf16().count() > js_string(&held["suffix"]).encode_utf16().count()) {
            winner = Some(rule);
        }
    }
    if let Some(rule) = winner {
        logical = slice(model, 0, model.len().saturating_sub(js_string(&rule["suffix"]).encode_utf16().count()));
        thinking = rule["thinking"] == true;
        if let Some(effort) = rule.get("effort") {
            out.set("effort", VariantSpec::from_json(effort).value);
        }
    } else {
        for lane in array(&vocabulary["lanes"]) {
            let suffix = js_string(&lane["suffix"]);
            if !contains(&lane["providers"], &normalized_provider) || !ends(&normalized, suffix) {
                continue;
            }
            let end = normalized.len() - suffix.encode_utf16().count();
            let normalized_trimmed = slice(&normalized, 0, end);
            let normalized_bare = bare(&normalized_trimmed);
            if lane.get("barePrefix").is_some_and(|p| !starts(&normalized_bare, js_string(p))) {
                continue;
            }
            let mut effort: Option<&Value> = None;
            for rule in array(&vocabulary["suffixes"]) {
                let suffix = js_string(&rule["suffix"]);
                if rule.get("effort").is_none()
                    || !ends(&normalized_trimmed, suffix)
                    || rule.get("exceptBarePrefix").is_some_and(|p| starts(&normalized_bare, js_string(p)))
                {
                    continue;
                }
                if effort.is_none_or(|held| suffix.len() > js_string(&held["suffix"]).len()) {
                    effort = Some(rule);
                }
            }
            let Some(effort) = effort else {
                continue;
            };
            let base = slice(model, 0, end.saturating_sub(js_string(&effort["suffix"]).encode_utf16().count()));
            if base.is_empty() || ends(&base, "/") {
                continue;
            }
            let mut units = base.units().to_vec();
            units.extend(slice(model, end, model.len()).units());
            logical = WireString::from_units(units);
            out.set("effort", VariantSpec::from_json(&effort["effort"]).value);
            break;
        }
    }
    out.set("logicalId", WireValue::String(logical));
    out.set("thinkingVariant", WireValue::Bool(thinking));
    out
}
pub fn classify_wire(provider: &WireString, model: &WireString, lenient: bool) -> Result<VariantSpec, CollapseError> {
    let trimmed = trim(model);
    let normalized_bare = lower(&bare(&trimmed));
    let normalized_provider = lower(provider);
    let mut agnostic = None;
    let mut specific = None;
    'classes: for class in array(&catalog_rules::compiled_rules()["taxonomy"]["classes"]) {
        for rule in array(&class["overrides"]) {
            if lower(&WireString::from(js_string(&rule["model"]))) != normalized_bare {
                continue;
            }
            if let Some(p) = rule.get("provider") {
                if lower(&WireString::from(js_string(p))) == normalized_provider {
                    specific = Some(rule);
                    break 'classes;
                }
            } else if agnostic.is_none() {
                agnostic = Some(rule);
            }
        }
    }
    if let Some(rule) = specific.or(agnostic) {
        let logical =
            rule.get("logical").and_then(Value::as_str).map(WireString::from).unwrap_or_else(|| trimmed.clone());
        let class = if let Some(class) = rule.get("class").and_then(Value::as_str) {
            class.to_owned()
        } else {
            get_string(&ranks(&logical, None, lenient)?, "class").to_utf8().unwrap()
        };
        let inferred = ranks(&logical, Some(&class), lenient)?;
        let mut out = object();
        out.set("class", s(&class));
        for key in ["family", "revision"] {
            if let Some(value) = rule.get(key).filter(|v| !v.is_null()) {
                out.set(key, VariantSpec::from_json(value).value);
            } else if let Some(value) = inferred.get(key) {
                out.set(key, value.clone());
            }
        }
        if let Some(value) = rule.get("effort") {
            out.set("effort", VariantSpec::from_json(value).value);
        }
        if rule["thinkingVariant"] == true {
            out.set("thinkingVariant", WireValue::Bool(true));
        }
        if logical != trimmed {
            out.set("logicalId", WireValue::String(logical));
        }
        return Ok(out);
    }
    let collapsed = if trimmed.len() == model.len() {
        collapsed_id(provider, &trimmed)
    } else {
        let mut out = object();
        out.set("logicalId", WireValue::String(trimmed.clone()));
        out.set("thinkingVariant", WireValue::Bool(false));
        out
    };
    let logical = get_string(&collapsed, "logicalId");
    let mut out = ranks(&logical, None, lenient)?;
    if let Some(effort) = collapsed.get("effort") {
        out.set("effort", effort.clone());
    }
    if collapsed.get("thinkingVariant") == Some(&WireValue::Bool(true)) {
        out.set("thinkingVariant", WireValue::Bool(true));
    }
    if logical != trimmed {
        out.set("logicalId", WireValue::String(logical));
    }
    Ok(out)
}

fn cascade(spec: &VariantSpec, identity: &VariantSpec) -> Result<Value, CollapseError> {
    let provider = get_string(spec, "provider");
    let model = get_string(spec, "id");
    let normalized = lower(&model);
    let revision = get_string(identity, "revision").to_utf8().ok().and_then(|v| catalog_rules::parse_revision(&v));
    type Rank = (u8, usize, f64);
    let mut matched = Vec::<(Rank, &Value)>::new();
    for rule in array(&catalog_rules::compiled_rules()["cascade"]["rules"]) {
        if rule.get("class").is_some_and(|v| !equal_ascii(&get_string(identity, "class"), js_string(v)))
            || rule.get("providers").is_some_and(|v| !contains(v, &provider))
            || rule.get("family").is_some_and(|v| !equal_ascii(&get_string(identity, "family"), js_string(v)))
        {
            continue;
        }
        if let Some(terms) = rule.get("revision") {
            let terms = array(terms)
                .iter()
                .flat_map(|term| {
                    let expression = format!("{}{}", js_string(&term["op"]), js_string(&term["revision"]));
                    catalog_rules::parse_revision_constraint(&expression).expect("compiled revision term")
                })
                .collect::<Vec<_>>();
            if !revision.is_some_and(|r| catalog_rules::revision_satisfies(r, &terms)) {
                continue;
            }
        }
        let mut exactness = 0;
        if let Some(selectors) = rule.get("models") {
            let mut best = None;
            for selector in array(selectors) {
                let value = js_string(&selector["value"]);
                let hit = match js_string(&selector["kind"]) {
                    "exact" => equal_ascii(&model, value),
                    "glob" => glob(value, &normalized),
                    "token" => normalized
                        .units()
                        .split(|u| !matches!(*u,48..=57|97..=122))
                        .any(|part| part == value.encode_utf16().collect::<Vec<_>>()),
                    _ => false,
                };
                if hit {
                    best = Some(best.unwrap_or(0).max(if selector["kind"] == "exact" { 2 } else { 1 }));
                }
            }
            let Some(best) = best else {
                continue;
            };
            exactness = best;
        }
        let dimensions = ["class", "providers", "family", "revision", "models"]
            .iter()
            .filter(|key| rule.get(**key).is_some())
            .count();
        matched.push(((exactness, dimensions, rule["priority"].as_f64().unwrap_or(0.0)), rule));
    }
    let reasoning = spec.get("reasoning").is_some_and(truthy)
        || matched.iter().any(|(rank, rule)| rank.0 == 2 && rule["thinking"].get("efforts").is_some());
    let mut out = json!({"wire":{},"thinking":{},"catalog":{}});
    for axis in ["wire", "thinking", "catalog"] {
        if axis == "thinking" && !reasoning {
            continue;
        }
        let mut winners = Vec::<(String, Rank, &Value)>::new();
        for (rank, rule) in &matched {
            let Some(values) = rule.get(axis).and_then(Value::as_object) else {
                continue;
            };
            for (key, _) in ara_prompt::js::entries(values) {
                if let Some((_, held, held_rule)) = winners.iter_mut().find(|(candidate, _, _)| candidate == key) {
                    if held == rank {
                        return Err(CollapseError::new(format!(
                            "ambiguous overlap on axis `{key}`: rules {} and {} tie",
                            held_rule["source"], rule["source"]
                        )));
                    }
                    if *held < *rank {
                        *held = *rank;
                        *held_rule = rule;
                    }
                } else {
                    winners.push((key.clone(), *rank, rule));
                }
            }
        }
        for (key, _, rule) in winners {
            out[axis][&key] = rule[axis][&key].clone();
        }
    }
    Ok(out)
}

// Object literal order from compat/resolve.ts. Undefined optional keys are own
// properties, including when the existing JSON detector omits their values.
const CHAT_KEYS: &[&str] = &[
    "supportsStore",
    "supportsDeveloperRole",
    "supportsMultipleSystemMessages",
    "supportsReasoningEffort",
    "supportsReasoningParams",
    "supportsSamplingParams",
    "supportsPenaltyAndStopParams",
    "reasoningEffortMap",
    "supportsUsageInStreaming",
    "alwaysSendMaxTokens",
    "disableReasoningOnForcedToolChoice",
    "disableReasoningOnToolChoice",
    "supportsToolChoice",
    "supportsForcedToolChoice",
    "supportsNamedToolChoice",
    "maxTokensField",
    "requiresToolResultName",
    "requiresAssistantAfterToolResult",
    "requiresThinkingAsText",
    "requiresMistralToolIds",
    "thinkingFormat",
    "kimiApiFormat",
    "reasoningDisableMode",
    "omitReasoningEffort",
    "includeEncryptedReasoning",
    "filterReasoningHistory",
    "thinkingKeep",
    "reasoningContentField",
    "requiresReasoningContentForToolCalls",
    "requiresReasoningContentForAllAssistantTurns",
    "allowsSyntheticReasoningContentForToolCalls",
    "replayReasoningContent",
    "qwenPreserveThinking",
    "qwenTemplateReasoningEffort",
    "requiresAssistantContentForToolCalls",
    "cacheControlFormat",
    "supportsPromptCacheBreakpoints",
    "promptCacheBreakpointTtl",
    "openRouterRouting",
    "vercelGatewayRouting",
    "isOpenRouterHost",
    "wireModelIdMode",
    "isVercelGatewayHost",
    "supportsStrictMode",
    "extraBody",
    "toolStrictMode",
    "toolSchemaFlavor",
    "streamFirstEventTimeoutMs",
    "streamIdleTimeoutMs",
    "stripDeepseekSpecialTokens",
    "streamMarkupHealingPattern",
    "reasoningDeltasMayBeCumulative",
    "emptyLengthFinishIsContextError",
    "usesOpenAIToolCallIdLimit",
    "promptCacheSessionHeader",
    "dropThinkingWhenReasoningEffort",
    "nativeKimiK3Reasoning",
    "zaiReasoningEffortDialect",
    "clampOutputToModelMax",
    "stripImageInput",
    "thinkingLoopGuard",
    "rejectRootObjectUnion",
    "retryWithoutStrictOnGrammarError",
    "supportsPromptCacheKey",
];
const RESPONSES_KEYS: &[&str] = &[
    "supportsDeveloperRole",
    "supportsStrictMode",
    "supportsReasoningEffort",
    "supportsLongPromptCacheRetention",
    "supportsPromptCacheBreakpoints",
    "promptCacheBreakpointTtl",
    "strictResponsesPairing",
    "supportsImageDetailOriginal",
    "supportsReasoningSummary",
    "supportsAllTurnsReasoningContext",
    "supportsConfigurationUpdate",
    "requiresReasoningOffJuiceInstruction",
    "stripImageInput",
    "thinkingLoopGuard",
    "reasoningEffortMap",
    "supportsReasoningParams",
    "supportsSamplingParams",
    "supportsPenaltyAndStopParams",
    "thinkingFormat",
    "reasoningDisableMode",
    "omitReasoningEffort",
    "includeEncryptedReasoning",
    "filterReasoningHistory",
    "disableReasoningOnForcedToolChoice",
    "disableReasoningOnToolChoice",
    "supportsToolChoice",
    "supportsForcedToolChoice",
    "supportsNamedToolChoice",
    "reasoningContentField",
    "requiresReasoningContentForToolCalls",
    "requiresReasoningContentForAllAssistantTurns",
    "allowsSyntheticReasoningContentForToolCalls",
    "replayReasoningContent",
    "qwenPreserveThinking",
    "qwenTemplateReasoningEffort",
    "requiresThinkingAsText",
    "requiresMistralToolIds",
    "requiresToolResultName",
    "requiresAssistantAfterToolResult",
    "requiresAssistantContentForToolCalls",
    "openRouterRouting",
    "vercelGatewayRouting",
    "isOpenRouterHost",
    "isVercelGatewayHost",
    "wireModelIdMode",
    "toolSchemaFlavor",
    "alwaysSendMaxTokens",
    "supportsObfuscationOptOut",
    "officialEndpoint",
    "harmonyLeakMitigation",
    "rejectRootObjectUnion",
    "retryWithoutStrictOnGrammarError",
    "cacheControlFormat",
    "stripDeepseekSpecialTokens",
    "streamMarkupHealingPattern",
    "reasoningDeltasMayBeCumulative",
    "emptyLengthFinishIsContextError",
    "usesOpenAIToolCallIdLimit",
    "promptCacheSessionHeader",
    "streamFirstEventTimeoutMs",
    "streamIdleTimeoutMs",
];
const RESPONSES_ONLY: &[&str] = &[
    "supportsLongPromptCacheRetention",
    "strictResponsesPairing",
    "supportsImageDetailOriginal",
    "supportsObfuscationOptOut",
    "supportsAllTurnsReasoningContext",
    "supportsConfigurationUpdate",
    "officialEndpoint",
    "harmonyLeakMitigation",
    "cacheControlFormat",
    "requiresReasoningOffJuiceInstruction",
    "supportsReasoningSummary",
    "isVercelGatewayHost",
];
fn compat_shape(api: &str, value: &Value) -> VariantSpec {
    let mut keys = match api {
        "openai-completions" | "openrouter" => CHAT_KEYS.to_vec(),
        "openai-responses" | "azure-openai-responses" | "openai-codex-responses" => RESPONSES_KEYS.to_vec(),
        "anthropic-messages" => vec![
            "officialEndpoint",
            "signingEndpoint",
            "supportsContextManagement",
            "supportsOutputEffort",
            "disableStrictTools",
            "disableAdaptiveThinking",
            "allowAnthropicHeaderOverrides",
            "supportsEagerToolInputStreaming",
            "supportsLongCacheRetention",
            "supportsMidConversationSystem",
            "supportsTurnScopedSystem",
            "supportsMidConversationToolChanges",
            "supportsPerMessageEffort",
            "supportsThinkingBindingControls",
            "supportsForcedToolChoice",
            "supportsSamplingParams",
            "requiresToolResultId",
            "requiresThinkingEnabled",
            "replayUnsignedThinking",
            "escapeBuiltinToolNames",
            "injectClaudeCodeInstruction",
            "stripImageInput",
            "thinkingLoopGuard",
            "streamIdleTimeoutMs",
        ],
        _ => value.as_object().map(|m| m.keys().map(String::as_str).collect()).unwrap_or_default(),
    };
    if api == "bedrock-converse-stream" && !keys.contains(&"streamIdleTimeoutMs") {
        keys.push("streamIdleTimeoutMs");
    }
    if let Some(values) = value.as_object() {
        for key in values.keys() {
            if !keys.contains(&key.as_str()) {
                keys.push(key);
            }
        }
    }
    let mut out = object();
    for key in keys {
        if let Some(value) = value.get(key) {
            if key == "whenThinking" && value.is_object() {
                out.set_record(key, &compat_shape("openai-completions", value));
            } else {
                out.set(key, VariantSpec::from_json(value).value);
            }
        } else {
            out.set_undefined(key);
        }
    }
    out
}
fn projection(value: &WireValue, path: &[WireString], undefined: &[Vec<WireString>]) -> Option<Value> {
    if undefined.iter().any(|p| p == path) {
        return None;
    }
    match value {
        WireValue::Null => Some(Value::Null),
        WireValue::Bool(v) => Some(json!(v)),
        WireValue::Number(v) => serde_json::Number::from_f64(*v).map(Value::Number),
        WireValue::String(v) => v.to_utf8().ok().map(Value::String),
        WireValue::Array(items) => Some(Value::Array(
            items
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let mut next = path.to_vec();
                    next.push(i.to_string().into());
                    projection(v, &next, undefined).unwrap_or(Value::Null)
                })
                .collect(),
        )),
        WireValue::Object(entries) => Some(Value::Object(
            entries
                .iter()
                .filter_map(|(key, v)| {
                    let key_utf8 = key.to_utf8().ok()?;
                    let mut next = path.to_vec();
                    next.push(key.clone());
                    Some((key_utf8, projection(v, &next, undefined)?))
                })
                .collect(),
        )),
    }
}
fn project(spec: &VariantSpec) -> Value {
    projection(&spec.value, &[], &spec.undefined_paths).unwrap_or_else(|| json!({}))
}
/// The detector observes provider only through fixed ASCII equality/prefixes,
/// and baseUrl through fixed ASCII marker/prefix/regex booleans or WHATWG URL.
/// A lone UTF-16 unit and U+FFFD are both outside every accepted ASCII class,
/// line separator and whitespace class in those probes. WHATWG URL itself
/// performs USVString conversion. This narrow view therefore preserves those
/// observations; it is never used for ids, authored records or returned data.
fn detector_input(spec: &VariantSpec) -> Value {
    let mut value = project(spec);
    for key in ["provider", "baseUrl"] {
        if let Some(WireValue::String(raw)) = spec.get(key)
            && raw.to_utf8().is_err()
        {
            value[key] = Value::String(String::from_utf16_lossy(raw.units()));
        }
    }
    value
}
fn resolved_thinking(
    spec: &VariantSpec,
    identity: &VariantSpec,
    axes: &Value,
    compat: Option<&VariantSpec>,
) -> Option<VariantSpec> {
    if !spec.get("reasoning").is_some_and(truthy) {
        return None;
    }
    let api = get_string(spec, "api");
    let cget = |key: &str| compat.and_then(|c| c.get(key));
    if (get_string(spec, "provider").equals_ascii("cline-pass")
        || ["openai-responses", "openai-codex-responses", "azure-openai-responses"].iter().any(|s| api.equals_ascii(s)))
        && cget("supportsReasoningEffort") == Some(&WireValue::Bool(false))
    {
        return None;
    }
    let raw = &axes["thinking"];
    let class = get_string(identity, "class");
    let family = get_string(identity, "family");
    let adaptive = |minimum: &str| {
        class.equals_ascii("anthropic")
            && ((family.equals_ascii("opus") && rev_gte(identity, minimum))
                || ["sonnet", "fable", "mythos"].iter().any(|s| family.equals_ascii(s)) && rev_gte(identity, "5"))
    };
    let default_display =
        ["anthropic-messages", "bedrock-converse-stream"].iter().any(|s| api.equals_ascii(s)) && adaptive("4.7");
    let display = raw.get("supportsDisplay").and_then(Value::as_bool).unwrap_or(default_display);
    let mandatory = identity.get("thinkingVariant") == Some(&WireValue::Bool(true))
        || strip_thinking_variant(&get_string(spec, "id")).is_some()
        || cget("qwenTemplateReasoningEffort") == Some(&WireValue::Bool(true));
    let requires = raw.get("requiresEffort").and_then(Value::as_bool).unwrap_or(mandatory);
    let rule_level = raw.get("defaultLevel").and_then(Value::as_str).filter(|s| catalog_rules_effort(s));
    let merge_map = |efforts: &WireValue| {
        let detected = ["openai-completions", "openrouter"].iter().any(|s| api.equals_ascii(s))
            && crate::model_identity::model_matches_host(&detector_input(spec), "fireworks");
        let configured = compat
            .and_then(|c| c.record("reasoningEffortMap"))
            .filter(|v| v.value.entries().is_some_and(|entries| !entries.is_empty()));
        let rule = raw.get("effortMap").and_then(Value::as_object);
        if !detected && configured.is_none() && rule.is_none() {
            return None;
        }
        let mut map = object();
        if detected {
            map.set("minimal", s("none"));
        }
        if let Some(rule) = rule {
            for (key, value) in rule {
                if catalog_rules_effort(key) && value.is_string() {
                    map.set(key, VariantSpec::from_json(value).value);
                }
            }
        }
        if let Some(configured) = configured {
            for key in configured.own_keys() {
                if let Ok(key) = key.to_utf8() {
                    if let Some(value) = configured.record(&key) {
                        map.set_record(&key, &value);
                    } else {
                        map.set_undefined(&key);
                    }
                }
            }
        }
        let mut filtered = object();
        for effort in efforts.as_array().unwrap_or_default() {
            let Some(effort) = effort.as_string().and_then(|s| s.to_utf8().ok()) else {
                continue;
            };
            if let Some(value) = map.record(&effort) {
                filtered.set_record(&effort, &value);
            }
        }
        (!filtered.own_keys().is_empty()).then_some(filtered)
    };
    if let Some(mut thinking) = spec
        .record("thinking")
        .filter(|t| t.get("efforts").and_then(WireValue::as_array).is_some_and(|efforts| !efforts.is_empty()))
    {
        let effort_map =
            if thinking.get("effortMap").is_none() { merge_map(thinking.get("efforts").unwrap()) } else { None };
        if let Some(map) = effort_map {
            thinking.set_record("effortMap", &map);
        }
        if thinking.get("supportsDisplay").is_none() && display {
            thinking.set("supportsDisplay", WireValue::Bool(true));
        }
        if thinking.get("defaultLevel").is_none()
            && let Some(level) = rule_level
        {
            thinking.set("defaultLevel", s(level));
        }
        if thinking.get("requiresEffort").is_none() && requires {
            thinking.set("requiresEffort", WireValue::Bool(true));
        }
        if thinking.get("prefixBinding").is_none() && raw["prefixBinding"] == true {
            thinking.set("prefixBinding", WireValue::Bool(true));
        }
        return Some(thinking);
    }
    if cget("trustExplicitThinkingOnly") == Some(&WireValue::Bool(true)) {
        return None;
    }
    let default_mode =
        if ["google-generative-ai", "google-gemini-cli", "google-vertex"].iter().any(|s| api.equals_ascii(s)) {
            if class.equals_ascii("gemini")
                && get_string(identity, "revision")
                    .to_utf8()
                    .ok()
                    .as_deref()
                    .and_then(catalog_rules::parse_revision)
                    .is_some_and(|r| r[0] == 3)
            {
                "google-level"
            } else {
                "budget"
            }
        } else if api.equals_ascii("anthropic-messages") {
            if class.equals_ascii("minimax") && ["m2", "m3"].iter().any(|s| family.equals_ascii(s)) {
                "anthropic-adaptive"
            } else if class.equals_ascii("glm")
                && rev_gte(identity, "5.2")
                && ["umans", "zai"].iter().any(|s| get_string(spec, "provider").equals_ascii(s))
            {
                "anthropic-budget-effort"
            } else if class.equals_ascii("anthropic") && rev_gte(identity, "4.6") && !family.equals_ascii("haiku") {
                "anthropic-adaptive"
            } else if class.equals_ascii("anthropic") && family.equals_ascii("opus") && rev_gte(identity, "4.5") {
                "anthropic-budget-effort"
            } else {
                "budget"
            }
        } else if api.equals_ascii("bedrock-converse-stream") {
            if adaptive("4.6") {
                "anthropic-adaptive"
            } else if class.equals_ascii("anthropic") && family.equals_ascii("opus") && rev_gte(identity, "4.5") {
                "anthropic-budget-effort"
            } else if class.equals_ascii("openai") {
                "effort"
            } else {
                "budget"
            }
        } else {
            "effort"
        };
    let mode =
        raw.get("mode").and_then(Value::as_str).filter(|s| model_policy::is_thinking_mode(s)).unwrap_or(default_mode);
    let xhigh = api.equals_ascii("anthropic-messages")
        || ["openai-responses", "openai-codex-responses", "azure-openai-responses"].iter().any(|s| api.equals_ascii(s))
        || ["openai-completions", "openrouter"].iter().any(|s| api.equals_ascii(s))
            && cget("thinkingFormat").and_then(WireValue::as_string).is_some_and(|s| s.equals_ascii("openai"))
            && cget("supportsReasoningEffort").is_some_and(truthy);
    let default_efforts = if xhigh {
        json!(["minimal", "low", "medium", "high", "xhigh"])
    } else {
        json!(["minimal", "low", "medium", "high"])
    };
    let efforts = raw
        .get("efforts")
        .filter(|v| {
            v.as_array().is_some_and(|items| {
                !items.is_empty() && items.iter().all(|v| v.as_str().is_some_and(catalog_rules_effort))
            })
        })
        .unwrap_or(&default_efforts);
    let mut thinking = object();
    thinking.set("mode", s(mode));
    thinking.set("efforts", VariantSpec::from_json(efforts).value);
    if let Some(level) = rule_level {
        thinking.set("defaultLevel", s(level));
    }
    if let Some(map) = merge_map(thinking.get("efforts").unwrap()) {
        thinking.set_record("effortMap", &map);
    }
    if let Some(budgets) = raw.get("effortBudgets").and_then(Value::as_object) {
        let mut record = object();
        for (key, value) in budgets {
            if catalog_rules_effort(key) && value.is_number() {
                record.set(key, VariantSpec::from_json(value).value);
            }
        }
        if !record.own_keys().is_empty() {
            thinking.set_record("effortBudgets", &record);
        }
    }
    if display {
        thinking.set("supportsDisplay", WireValue::Bool(true));
    }
    if raw["prefixBinding"] == true {
        thinking.set("prefixBinding", WireValue::Bool(true));
    }
    if requires {
        thinking.set("requiresEffort", WireValue::Bool(true));
    }
    if raw["suppressWhenOff"] == true {
        thinking.set("suppressWhenOff", WireValue::Bool(true));
    }
    Some(thinking)
}
fn catalog_rules_effort(value: &str) -> bool {
    matches!(value, "minimal" | "low" | "medium" | "high" | "xhigh" | "max")
}
// Native stages mirror resolve.ts: detector/axes -> overrides -> map overlay ->
// base fixups -> ordinary-object thinking clone -> thinking overrides/fixups.
// Prototypes remain a lookup view; exporting or spreading copies only own keys.
const OBJECT_PROTOTYPE_KEYS: &[&str] = &[
    "constructor",
    "__defineGetter__",
    "__defineSetter__",
    "hasOwnProperty",
    "__lookupGetter__",
    "__lookupSetter__",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toString",
    "valueOf",
    "__proto__",
    "toLocaleString",
];
#[derive(Clone)]
enum NativePrototype {
    Ordinary,
    Authored(VariantSpec),
    Null,
}
#[derive(Clone)]
struct NativeCompat {
    own: VariantSpec,
    prototype: NativePrototype,
}
fn enumerable_keys(record: &VariantSpec) -> Vec<WireString> {
    match &record.value {
        WireValue::Object(_) => record.own_keys(),
        WireValue::Array(items) => (0..items.len()).map(|i| i.to_string().into()).collect(),
        WireValue::String(value) => (0..value.len()).map(|i| i.to_string().into()).collect(),
        _ => Vec::new(),
    }
}
fn own_property(record: &VariantSpec, key: &WireString) -> Option<VariantSpec> {
    if let WireValue::String(value) = &record.value {
        let index = key.to_utf8().ok()?.parse::<usize>().ok()?;
        return value
            .units()
            .get(index)
            .map(|unit| VariantSpec::from_wire(WireValue::String(WireString::from_units(vec![*unit]))));
    }
    record.record_key(key)
}
fn spread_into(out: &mut VariantSpec, record: &VariantSpec) {
    for key in enumerable_keys(record) {
        if let Some(value) = own_property(record, &key) {
            out.set_key_record(&key, &value);
        } else {
            out.set_key_undefined(&key);
        }
    }
}
impl NativePrototype {
    fn has(&self, key: &WireString) -> bool {
        match self {
            Self::Null => false,
            Self::Ordinary => key.to_utf8().is_ok_and(|key| OBJECT_PROTOTYPE_KEYS.contains(&key.as_str())),
            Self::Authored(record) => {
                enumerable_keys(record).contains(key)
                    || key.to_utf8().is_ok_and(|key| {
                        OBJECT_PROTOTYPE_KEYS.contains(&key.as_str())
                            || record
                                .value
                                .as_array()
                                .is_some_and(|items| model_policy::array_prototype_has_key(&key, items.len()))
                    })
            }
        }
    }
    fn proto_is_accessor(&self) -> bool {
        !matches!(self, Self::Null)
            && !matches!(self, Self::Authored(record) if record.own_keys().iter().any(|k| k.equals_ascii("__proto__")))
    }
    fn record(&self, key: &WireString) -> Option<VariantSpec> {
        if let Self::Authored(record) = self {
            if key.equals_ascii("length")
                && let Some(items) = record.value.as_array()
            {
                return Some(VariantSpec::from_wire(WireValue::Number(items.len() as f64)));
            }
            return own_property(record, key);
        }
        None
    }
}
impl NativeCompat {
    fn new(own: VariantSpec) -> Self {
        Self { own, prototype: NativePrototype::Ordinary }
    }
    fn record(&self, key: &str) -> Option<VariantSpec> {
        let key = WireString::from(key);
        if self.own.own_keys().contains(&key) { self.own.record_key(&key) } else { self.prototype.record(&key) }
    }
    fn overrides(&mut self, overrides: Option<&VariantSpec>) {
        let Some(overrides) = overrides.filter(|record| truthy(&record.value)) else { return };
        for key in enumerable_keys(overrides) {
            let Some(value) = own_property(overrides, &key) else { continue };
            let own = self.own.own_keys().contains(&key);
            if !own && !self.prototype.has(&key) {
                continue;
            }
            if key.equals_ascii("__proto__") && !own && self.prototype.proto_is_accessor() {
                match &value.value {
                    WireValue::Null => self.prototype = NativePrototype::Null,
                    WireValue::Object(_) | WireValue::Array(_) => self.prototype = NativePrototype::Authored(value),
                    _ => {}
                }
            } else {
                self.own.set_key_record(&key, &value);
            }
        }
    }
    fn effective(&self) -> VariantSpec {
        let mut out = object();
        if let NativePrototype::Authored(proto) = &self.prototype {
            spread_into(&mut out, proto);
            if let Some(items) = proto.value.as_array() {
                out.set("length", WireValue::Number(items.len() as f64));
            }
        }
        spread_into(&mut out, &self.own);
        out
    }
}
fn axis_effort_map(axes: &Value) -> Option<VariantSpec> {
    let mut map = object();
    for (key, value) in axes["wire"]["reasoningEffortMap"].as_object()? {
        if catalog_rules_effort(key) && value.is_string() {
            map.set(key, VariantSpec::from_json(value).value);
        }
    }
    (!map.own_keys().is_empty()).then_some(map)
}
fn overlay_native_effort_map(compat: &mut NativeCompat, authored: Option<&VariantSpec>, axes: &Value) {
    if let Some(mut axis) = axis_effort_map(axes)
        && let Some(raw) = authored.and_then(|c| c.record("reasoningEffortMap")).filter(|c| truthy(&c.value))
    {
        spread_into(&mut axis, &raw);
        compat.own.set_record("reasoningEffortMap", &axis);
    }
}
fn native_disable_mode(compat: &NativeCompat) -> &'static str {
    let format = compat.record("thinkingFormat");
    let value = format.as_ref().and_then(|r| r.value.as_string()).and_then(|s| s.to_utf8().ok());
    model_policy::reasoning_disable_mode(value.as_deref().unwrap_or(""))
}
fn fixup_native_openai(
    spec: &VariantSpec,
    identity: &VariantSpec,
    axes: &Value,
    compat: &mut NativeCompat,
    completions: bool,
) {
    let authored = spec.record("compat");
    let view = detector_input(spec);
    let host = |name| crate::model_identity::model_matches_host(&view, name);
    let direct = completions
        && host("deepseekDirect")
        && (get_string(identity, "class").equals_ascii("deepseek") || host("deepseekFamily"))
        && spec.get("reasoning").is_some_and(truthy);
    let venice = host("venice");
    if !completions && host("xai") && axes["wire"].get("reasoningEffortMap").is_some() {
        let canonical = axis_effort_map(axes).unwrap_or_else(object);
        let mut mapped = object();
        if let Some(current) = compat.record("reasoningEffortMap") {
            spread_into(&mut mapped, &current);
        }
        spread_into(&mut mapped, &canonical);
        for key in ["xhigh", "max"] {
            if canonical.get(key).is_none() {
                mapped.remove(key);
            }
        }
        compat.own.set_record("reasoningEffortMap", &mapped);
    }
    if direct && let Some(body) = compat.record("extraBody") {
        let thinking = body.record("thinking");
        if thinking.as_ref().is_some_and(|t| t.value.is_object() && get_string(t, "type").equals_ascii("enabled")) {
            let mut cleaned = object();
            spread_into(&mut cleaned, &body);
            cleaned.remove("thinking");
            if cleaned.own_keys().is_empty() {
                compat.own.set_undefined("extraBody");
            } else {
                compat.own.set_record("extraBody", &cleaned);
            }
        }
    }
    if authored.as_ref().and_then(|c| c.get("reasoningDisableMode")).is_none()
        && axes["wire"].get("reasoningDisableMode").is_none()
    {
        let mode = if completions && get_string(spec, "provider").equals_ascii("cline-pass") {
            "cline-enabled-false"
        } else if completions
            && host("moonshotNative")
            && get_string(identity, "class").equals_ascii("kimi")
            && get_string(identity, "family").equals_ascii("k2.7-code")
        {
            "omit"
        } else if direct {
            "zai-thinking-disabled"
        } else if completions && venice {
            "venice-disable-thinking"
        } else {
            native_disable_mode(compat)
        };
        compat.own.set("reasoningDisableMode", s(mode));
    }
    if authored.as_ref().and_then(|c| c.get("omitReasoningEffort")).is_none()
        && axes["wire"].get("omitReasoningEffort").is_none()
        && !compat.record("supportsReasoningEffort").is_some_and(|c| truthy(&c.value))
    {
        compat.own.set("omitReasoningEffort", WireValue::Bool(true));
    }
    if !completions {
        if get_string(spec, "provider").equals_ascii("xai-oauth")
            && axes["wire"]["supportsReasoningEffort"] == true
            && authored.as_ref().and_then(|c| c.get("supportsReasoningEffort")) != Some(&WireValue::Bool(false))
        {
            compat.own.set("supportsReasoningEffort", WireValue::Bool(true));
            compat.own.set("omitReasoningEffort", WireValue::Bool(false));
        }
        return;
    }
    let mut when =
        authored.as_ref().and_then(|c| c.record("whenThinking")).filter(|r| !matches!(r.value, WireValue::Null));
    if when.is_none() && spec.get("reasoning").is_some_and(truthy) && axes["wire"]["whenThinking"].is_object() {
        when = Some(VariantSpec::from_json(&axes["wire"]["whenThinking"]));
    }
    if when.is_none() && direct {
        let mut extra = object();
        if let Some(body) = compat.record("extraBody") {
            spread_into(&mut extra, &body);
        }
        extra.set("thinking", VariantSpec::from_json(&json!({"type":"enabled"})).value);
        let mut fallback = object();
        fallback.set_record("extraBody", &extra);
        when = Some(fallback);
    }
    if let Some(when) = when.filter(|r| truthy(&r.value)) {
        let mut variant = NativeCompat::new(compat.own.clone());
        variant.own.set_undefined("whenThinking");
        variant.overrides(Some(&when));
        if when.get("reasoningDisableMode").is_none() {
            variant.own.set(
                "reasoningDisableMode",
                s(if venice { "venice-disable-thinking" } else { native_disable_mode(&variant) }),
            );
        }
        if when.get("omitReasoningEffort").is_none()
            && !variant.record("supportsReasoningEffort").is_some_and(|c| truthy(&c.value))
        {
            variant.own.set("omitReasoningEffort", WireValue::Bool(true));
        }
        compat.own.set_record("whenThinking", &variant.own);
    } else {
        compat.own.set_undefined("whenThinking");
    }
}
fn native_compat(
    spec: &VariantSpec,
    identity: &VariantSpec,
    axes: &Value,
    input: &Value,
    identity_json: &Value,
    api: &str,
    completions: bool,
) -> Option<NativeCompat> {
    let seed = model_policy::compat_seed_with_axes(input, identity_json, axes, api)?;
    let shape_api = if completions {
        "openai-completions"
    } else if api == "openrouter" {
        "openai-responses"
    } else {
        api
    };
    let mut compat = NativeCompat::new(compat_shape(shape_api, &seed));
    let authored = spec.record("compat");
    compat.overrides(authored.as_ref());
    if completions
        || ["openrouter", "openai-responses", "azure-openai-responses", "openai-codex-responses"].contains(&api)
    {
        overlay_native_effort_map(&mut compat, authored.as_ref(), axes);
        fixup_native_openai(spec, identity, axes, &mut compat, completions);
    }
    Some(compat)
}
impl CollapseModelPolicy for WireModelPolicy {
    fn resolve(&self, spec: &VariantSpec) -> Result<VariantSpec, CollapseError> {
        let identity = classify_wire(&get_string(spec, "provider"), &get_string(spec, "id"), false)?;
        let axes = cascade(spec, &identity)?;
        let mut identity_json = project(&identity);
        identity_json.as_object_mut().unwrap().remove("logicalId");
        let mut input = detector_input(spec);
        input["reasoning"] = json!(spec.get("reasoning").is_some_and(truthy));
        let api = get_string(spec, "api").to_utf8().unwrap_or_default();
        let mut compat = if api == "openrouter" {
            let mut chat = native_compat(spec, &identity, &axes, &input, &identity_json, "openai-completions", true);
            let responses = native_compat(spec, &identity, &axes, &input, &identity_json, "openrouter", false);
            if let (Some(chat), Some(responses)) = (&mut chat, responses) {
                for key in RESPONSES_ONLY {
                    if let Some(value) = responses.record(key) {
                        chat.own.set_record(key, &value);
                    } else {
                        chat.own.set_undefined(key);
                    }
                }
                chat.prototype = NativePrototype::Ordinary;
            }
            chat
        } else {
            native_compat(spec, &identity, &axes, &input, &identity_json, &api, api == "openai-completions")
        };
        let mut out = object();
        out.set_record("identity", &identity);
        let effective = compat.as_ref().map(NativeCompat::effective);
        if let Some(compat) = compat.take() {
            out.set_record("compat", &compat.own);
        } else {
            out.set_undefined("compat");
        }
        if let Some(thinking) = resolved_thinking(spec, &identity, &axes, effective.as_ref()) {
            out.set_record("thinking", &thinking);
        } else {
            out.set_undefined("thinking");
        }
        out.set("catalog", VariantSpec::from_json(&axes["catalog"]).value);
        Ok(out)
    }
    fn build(&self, spec: &VariantSpec) -> Result<VariantSpec, CollapseError> {
        let policy = self.resolve(spec)?;
        let identity = policy.record("identity").unwrap();
        let name = spec
            .get("name")
            .and_then(WireValue::as_string)
            .ok_or_else(|| CollapseError::new("Model name must be a string".into()))?;
        let has_config = spec.own_keys().iter().any(|key| key.equals_ascii("supportsComputerUseConfig"));
        let explicit = if has_config {
            spec.get("supportsComputerUseConfig").filter(|v| matches!(v, WireValue::Bool(_))).cloned()
        } else {
            spec.get("supportsComputerUse").cloned()
        };
        let support = if let Some(explicit) = &explicit {
            explicit.clone()
        } else {
            let wire_identity = if spec.get("requestModelId").is_some() {
                classify_wire(&get_string(spec, "provider"), &get_string(spec, "requestModelId"), false)?
            } else {
                identity.clone()
            };
            WireValue::Bool(
                model_policy::direct_openai_responses_endpoint(&detector_input(spec))
                    && get_string(&wire_identity, "class").equals_ascii("openai")
                    && rev_gte(&wire_identity, "5.4"),
            )
        };
        let mut model = spec.clone();
        model.set("name", WireValue::String(clean_name(name)?));
        model.set_record("identity", &identity);
        model.set(
            "requiresGlyphTokenization",
            WireValue::Bool(get_string(&identity, "class").equals_ascii("anthropic")),
        );
        let tokenizer = spec.get("tokenizer").filter(|v| !matches!(v, WireValue::Null)).cloned().or_else(|| {
            let id = spec
                .get("requestModelId")
                .filter(|v| !matches!(v, WireValue::Null))
                .and_then(WireValue::as_string)
                .cloned()
                .unwrap_or_else(|| get_string(spec, "id"));
            tokenizer(&id).map(s)
        });
        if let Some(tokenizer) = tokenizer {
            model.set("tokenizer", tokenizer);
        } else {
            model.set_undefined("tokenizer");
        }
        if let Some(thinking) = policy.record("thinking") {
            model.set_record("thinking", &thinking);
        } else {
            model.set_undefined("thinking");
        }
        model.set("supportsComputerUse", support);
        if let Some(explicit) = explicit {
            model.set("supportsComputerUseConfig", explicit);
        } else {
            model.set_undefined("supportsComputerUseConfig");
        }
        if let Some(compat) = policy.record("compat") {
            model.set_record("compat", &compat);
        } else {
            model.set_undefined("compat");
        }
        if let Some(compat) = spec.record("compat") {
            model.set_record("compatConfig", &compat);
        } else {
            model.set_undefined("compatConfig");
        }
        apply_catalog(&mut model, &policy.record("catalog").unwrap());
        Ok(model)
    }
}
fn rev_gte(identity: &VariantSpec, bound: &str) -> bool {
    get_string(identity, "revision")
        .to_utf8()
        .ok()
        .as_deref()
        .and_then(catalog_rules::parse_revision)
        .zip(catalog_rules::parse_revision(bound))
        .is_some_and(|(a, b)| catalog_rules::compare_revision(a, b) >= 0)
}
fn tokenizer(id: &WireString) -> Option<&'static str> {
    let identity = classify_wire(&WireString::from(""), &bare(id), true).ok()?;
    let class = get_string(&identity, "class");
    let family = get_string(&identity, "family");
    if class.equals_ascii("anthropic") {
        Some(if family.equals_ascii("opus") {
            if rev_gte(&identity, "5") {
                "claude-v5"
            } else if rev_gte(&identity, "4.7") {
                "claude-v47"
            } else {
                "claude-v3"
            }
        } else if ["sonnet", "fable", "mythos"].iter().any(|s| family.equals_ascii(s)) && rev_gte(&identity, "5") {
            "claude-v5-sonnet"
        } else {
            "claude-v3"
        })
    } else if class.equals_ascii("qwen") && rev_gte(&identity, "3.5") {
        Some("qwen3")
    } else if class.equals_ascii("deepseek") {
        Some("deepseek-v3")
    } else if class.equals_ascii("kimi") {
        Some("kimi-k2")
    } else if class.equals_ascii("glm") && rev_gte(&identity, "5") {
        Some("glm5")
    } else {
        None
    }
}
fn replace_regex(
    value: &WireString,
    pattern: &str,
    flags: &str,
    replacement: &str,
) -> Result<WireString, CollapseError> {
    let mut regex = JsRegExp::new(pattern.into(), flags).map_err(|e| CollapseError::new(e.to_string()))?;
    let mut cursor = 0;
    let mut out = Vec::new();
    while let Some(hit) = regex.exec(value) {
        out.extend(&value.units()[cursor..hit.index]);
        out.extend(replacement.encode_utf16());
        cursor = hit.end;
        if !flags.contains('g') {
            break;
        }
    }
    out.extend(&value.units()[cursor..]);
    Ok(WireString::from_units(out))
}
fn clean_name(name: &WireString) -> Result<WireString, CollapseError> {
    let cleaned = replace_regex(name, r"^[A-Za-z][A-Za-z0-9 .+&'-]{0,23}: ", "", "")?;
    let cleaned = replace_regex(&cleaned, r"\s*\((?:latest|Antigravity|\$+|>?\d+% off|retires [^)]*)\)", "g", "")?;
    let cleaned = trim(&replace_regex(&cleaned, " {2,}", "g", " ")?);
    Ok(if cleaned.is_empty() { name.clone() } else { cleaned })
}
fn number(spec: &VariantSpec, key: &str) -> Option<f64> {
    match spec.get(key) {
        Some(WireValue::Number(v)) => Some(*v),
        _ => None,
    }
}
fn apply_catalog(model: &mut VariantSpec, catalog: &VariantSpec) {
    if let Some(tier) = catalog.record("serviceTierCost").filter(|v| v.value.is_object()) {
        let mut out = object();
        for key in ["flex", "priority"] {
            if let Some(v) = number(&tier, key) {
                out.set(key, WireValue::Number(v));
            }
        }
        model.set_record("serviceTierCost", &out);
    }
    if let Some(v) = number(catalog, "priority") {
        model.set("priority", WireValue::Number(v));
    }
    if let Some(v) = catalog
        .get("applyPatchToolType")
        .filter(|v| v.as_string().is_some_and(|s| s.equals_ascii("freeform") || s.equals_ascii("function")))
    {
        model.set("applyPatchToolType", v.clone());
    }
    if catalog.get("requiresCursorToolSchemaProjection") == Some(&WireValue::Bool(true)) {
        model.set("requiresCursorToolSchemaProjection", WireValue::Bool(true));
    } else {
        model.remove("requiresCursorToolSchemaProjection");
    }
    if model.get("contextPromotionTarget").is_none()
        && let Some(v) = catalog.get("contextPromotionTarget").filter(|v| v.as_string().is_some())
    {
        model.set("contextPromotionTarget", v.clone());
    }
    if let Some(long) = catalog.record("longContext").filter(|v| v.value.is_object()) {
        let mut cost = model.record("cost").unwrap_or_else(object);
        let prices = ["input", "output", "cacheRead", "cacheWrite"];
        let has_price = prices.iter().any(|key| number(&cost, key) != Some(0.0));
        if let (Some(threshold), Some(multiplier)) = (number(&long, "inputThreshold"), number(&long, "multiplier"))
            && has_price
        {
            let mut tier = object();
            tier.set("inputThreshold", WireValue::Number(threshold));
            if long.get("inputThresholdInclusive") == Some(&WireValue::Bool(true)) {
                tier.set("inputThresholdInclusive", WireValue::Bool(true));
            }
            for key in prices {
                tier.set(key, WireValue::Number(number(&cost, key).unwrap_or(f64::NAN) * multiplier));
            }
            cost.set_record("longContext", &tier);
            model.set_record("cost", &cost);
        } else if number(&long, "inputThreshold").is_some() && prices.iter().all(|key| number(&long, key).is_some()) {
            let mut tier = object();
            for key in ["inputThreshold", "input", "output", "cacheRead", "cacheWrite"] {
                tier.set(key, long.get(key).unwrap().clone());
            }
            cost.set_record("longContext", &tier);
            model.set_record("cost", &cost);
        }
    }
    if let Some(patch) = catalog.record("costPatch").filter(|v| v.value.is_object()) {
        let mut cost = model.record("cost").unwrap_or_else(object);
        for key in ["input", "output", "cacheRead", "cacheWrite"] {
            if let Some(v) = number(&patch, key) {
                cost.set(key, WireValue::Number(v));
            }
        }
        model.set_record("cost", &cost);
    }
    if let Some(patch) = catalog.record("limitsPatch").filter(|v| v.value.is_object()) {
        for key in ["contextWindow", "maxTokens"] {
            if let Some(v) = number(&patch, key) {
                model.set(key, WireValue::Number(v));
            }
        }
    }
    if let Some(floor) = number(catalog, "contextWindowFloor") {
        let live = match model.get("contextWindow") {
            None | Some(WireValue::Null) => 0.0,
            Some(WireValue::Number(v)) => *v,
            _ => f64::NAN,
        };
        let maximum = if live.is_nan() || floor.is_nan() {
            f64::NAN
        } else if live == 0.0 && floor == 0.0 {
            if live.is_sign_positive() || floor.is_sign_positive() { 0.0 } else { -0.0 }
        } else {
            live.max(floor)
        };
        model.set("contextWindow", WireValue::Number(maximum));
    }
    if let Some(input) = catalog.get("inputModalities").filter(|v| {
        v.as_array().is_some_and(|items| {
            items.iter().all(|v| v.as_string().is_some_and(|s| s.equals_ascii("text") || s.equals_ascii("image")))
        })
    }) {
        model.set("input", input.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source_spec(provider: &str, id: &str, api: &str) -> VariantSpec {
        VariantSpec::from_json(&json!({"provider":provider,"id":id,"name":id,"api":api,
            "baseUrl":"https://example.invalid/v1","reasoning":true,"input":["text"],
            "cost":{"input":1,"output":2,"cacheRead":0,"cacheWrite":0},"contextWindow":32768,"maxTokens":4096}))
    }
    #[test]
    fn ambiguity_error_message_preserves_lone_utf16_without_escape_aliasing() {
        let mut units = "mistral-mixtral".encode_utf16().collect::<Vec<_>>();
        units.push(0xd800);
        let error = classify_wire(&"fixture".into(), &WireString::from_units(units), false).unwrap_err();
        assert_eq!(error.name(), "AmbiguousIdentityError");
        let mut expected = "ambiguous family for `mistral-mixtral".encode_utf16().collect::<Vec<_>>();
        expected.push(0xd800);
        expected.extend("`: `mistral` and `mixtral` tie".encode_utf16());
        assert_eq!(error.message_wire(), WireString::from_units(expected));
        let literal =
            CollapseError::new("ambiguous family for `mistral-mixtral\\ud800`: `mistral` and `mixtral` tie".into());
        assert_ne!(error.message_wire(), literal.message_wire());
    }
    // Expectations were observed on the retained, SHA-guarded fixed OMP
    // resolve.ts with Bun 1.4.0; these are source regressions, not JSON fixtures.
    #[test]
    fn deepseek_body_cleanup_precedes_authored_thinking_clone() {
        let mut spec = source_spec("deepseek", "deepseek-reasoner", "openai-completions");
        let mut authored = VariantSpec::from_json(&json!({"whenThinking":{"thinkingFormat":"openai"}}));
        let mut body = VariantSpec::from_json(&json!({"thinking":{"type":"enabled"}}));
        body.set_undefined("opaque");
        authored.set_record("extraBody", &body);
        spec.set_record("compat", &authored);
        let compat = WireModelPolicy.resolve(&spec).unwrap().record("compat").unwrap();
        for row in [compat.clone(), compat.record("whenThinking").unwrap()] {
            let body = row.record("extraBody").unwrap();
            assert_eq!(body.own_keys(), vec![WireString::from("opaque")]);
            assert!(body.get("opaque").is_none());
            assert!(body.get("thinking").is_none());
        }
    }
    #[test]
    fn xai_canonical_map_spread_keeps_native_undefined_and_accepts_null() {
        let mut spec = source_spec("xai", "grok-4", "openai-responses");
        let mut authored = object();
        let mut map = object();
        map.set_undefined("extra");
        map.set("low", s("old"));
        map.set_undefined("xhigh");
        authored.set_record("reasoningEffortMap", &map);
        spec.set_record("compat", &authored);
        let compat = WireModelPolicy.resolve(&spec).unwrap().record("compat").unwrap();
        let map = compat.record("reasoningEffortMap").unwrap();
        assert_eq!(map.own_keys(), ["minimal", "xhigh", "max", "extra", "low"].map(WireString::from));
        assert_eq!(map.get("minimal"), Some(&s("low")));
        assert_eq!(map.get("xhigh"), Some(&s("high")));
        assert_eq!(map.get("max"), Some(&s("high")));
        assert_eq!(map.get("low"), Some(&s("old")));
        assert!(map.get("extra").is_none());
        authored.set("reasoningEffortMap", WireValue::Null);
        spec.set_record("compat", &authored);
        let map =
            WireModelPolicy.resolve(&spec).unwrap().record("compat").unwrap().record("reasoningEffortMap").unwrap();
        assert_eq!(map.own_keys(), ["minimal", "xhigh", "max"].map(WireString::from));
    }
    #[test]
    fn native_prototype_membership_is_separate_from_exported_own_keys() {
        let mut spec = source_spec("fixture", "novel", "openai-completions");
        let authored = VariantSpec::from_wire(WireValue::parse(
            r#"{"__proto__":{"trustExplicitThinkingOnly":true,"junk":true,"\ud800":true},"junk":"\ud800","\ud800":"\udfff"}"#,
        ).unwrap());
        spec.set_record("compat", &authored);
        let policy = WireModelPolicy.resolve(&spec).unwrap();
        assert!(policy.get("thinking").is_none());
        let compat = policy.record("compat").unwrap();
        assert!(!compat.own_keys().iter().any(|key| key.equals_ascii("trustExplicitThinkingOnly")));
        assert_eq!(compat.get("junk"), Some(&WireValue::String(WireString::from_units(vec![0xd800]))));
        assert_eq!(
            compat.get_path(&[WireString::from_units(vec![0xd800])]),
            Some(&WireValue::String(WireString::from_units(vec![0xdfff])))
        );
        let keys = compat.own_keys();
        assert!(
            keys.iter().position(|key| key.equals_ascii("junk")).unwrap()
                < keys.iter().position(|key| key.equals_ascii("whenThinking")).unwrap()
        );
        let authored = VariantSpec::from_wire(WireValue::parse(r#"{"__proto__":[true],"at":"\ud800"}"#).unwrap());
        spec.set_record("compat", &authored);
        let compat = WireModelPolicy.resolve(&spec).unwrap().record("compat").unwrap();
        assert_eq!(compat.get("at"), Some(&WireValue::String(WireString::from_units(vec![0xd800]))));
        assert!(!compat.own_keys().iter().any(|key| key.equals_ascii("0")));
    }
    #[test]
    fn undefined_effort_mapping_is_filtered_and_lone_url_keeps_host_facts() {
        let mut spec = source_spec("fixture", "novel", "openai-completions");
        let mut authored = object();
        let mut map = object();
        map.set_undefined("low");
        map.set("extra", s("unused"));
        authored.set_record("reasoningEffortMap", &map);
        spec.set_record("compat", &authored);
        let policy = WireModelPolicy.resolve(&spec).unwrap();
        assert!(policy.record("thinking").unwrap().get("effortMap").is_none());
        assert!(
            policy.record("compat").unwrap().record("reasoningEffortMap").unwrap().own_keys().contains(&"low".into())
        );
        for (prefix, tool_choice, replay) in
            [("https://api.deepseek.com/v1/", false, false), ("http://127.0.0.1:8000/v1/", true, true)]
        {
            let mut spec = source_spec("fixture", "deepseek-reasoner", "openai-completions");
            let mut units = prefix.encode_utf16().collect::<Vec<_>>();
            units.push(0xd800);
            spec.set("baseUrl", WireValue::String(WireString::from_units(units)));
            let built = WireModelPolicy.build(&spec).unwrap();
            assert_eq!(built.get("baseUrl"), spec.get("baseUrl"));
            let compat = built.record("compat").unwrap();
            assert_eq!(compat.get("supportsToolChoice"), Some(&WireValue::Bool(tool_choice)));
            assert_eq!(compat.get("replayReasoningContent"), Some(&WireValue::Bool(replay)));
            assert_eq!(compat.get("streamIdleTimeoutMs"), Some(&WireValue::Number(300_000.0)));
        }
    }
    #[test]
    fn native_build_keeps_lone_strings_numbers_and_own_undefined() {
        let mut spec = VariantSpec::from_json(
            &json!({"id":"gpt-5.4","name":"OpenAI: Name (latest)","provider":"fixture","api":"openai-completions","reasoning":true,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0},"contextWindow":32768,"maxTokens":4096}),
        );
        spec.set("name", WireValue::String(WireString::from_units(vec![b'n' as u16, 0xd800])));
        spec.set("contextWindow", WireValue::Number(f64::NAN));
        spec.set("maxTokens", WireValue::Number(f64::INFINITY));
        spec.set_undefined("supportsComputerUseConfig");
        let built = WireModelPolicy.build(&spec).unwrap();
        assert!(number(&built, "contextWindow").unwrap().is_nan());
        assert_eq!(built.get("name"), spec.get("name"));
        let compat = built.record("compat").unwrap();
        assert!(compat.own_keys().iter().any(|k| k.equals_ascii("kimiApiFormat")));
        assert!(compat.get("kimiApiFormat").is_none());
    }
    #[test]
    fn native_identity_preserves_original_utf16_logical_id() {
        let id = WireString::from_units(vec![
            b'g' as u16,
            b'p' as u16,
            b't' as u16,
            b'-' as u16,
            b'5' as u16,
            b'.' as u16,
            b'4' as u16,
            0xd800,
            b'-' as u16,
            b'h' as u16,
            b'i' as u16,
            b'g' as u16,
            b'h' as u16,
        ]);
        let identity = classify_wire(&"fixture".into(), &id, false).unwrap();
        assert!(get_string(&identity, "class").equals_ascii("openai"));
        assert_eq!(get_string(&identity, "logicalId"), slice(&id, 0, id.len() - 5));
    }
}
