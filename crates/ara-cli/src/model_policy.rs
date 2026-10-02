//! Native catalog policy, ported from OMP 596f2da7101178214aa27a753529d15e6b7ad91d.
//!
//! Source: packages/catalog/src/{build,model-tokenizer,utils}.ts,
//! compat/{resolve,axes,apply,anthropic,openai}.ts and coding-agent
//! session/role-models.ts plus config/model-resolver.ts. Missing JSON properties
//! represent JavaScript `undefined`; authored nulls remain authored nulls.
//! The utils port here covers pure value helpers. `discoveryFetch` and its
//! per-request extra-CA transport composition belong to discovery transport.
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

use std::{collections::BTreeSet, fmt, sync::OnceLock};

use regex::Regex;
use serde_json::{Map, Value, json};

use crate::{
    catalog_rules::{
        ClassifyOptions, Revision, classify_model, compare_revision, parse_revision, resolve_cascade,
        strip_thinking_variant_suffix_utf16,
    },
    model_identity::{host_matches_url, model_matches_host},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogPolicyError(pub String);

impl fmt::Display for CatalogPolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for CatalogPolicyError {}

const EFFORTS: &[&str] = &["minimal", "low", "medium", "high", "xhigh", "max"];
const DEFAULT_EFFORTS: &[&str] = &["minimal", "low", "medium", "high"];
const DEFAULT_EFFORTS_XHIGH: &[&str] = &["minimal", "low", "medium", "high", "xhigh"];
const THINKING_MODES: &[&str] = &["effort", "budget", "google-level", "anthropic-adaptive", "anthropic-budget-effort"];

fn string<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(v) => *v,
        Value::Number(v) => v.as_f64().is_some_and(|v| v != 0.0 && !v.is_nan()),
        Value::String(v) => !v.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}
fn flag(v: &Value, key: &str) -> bool {
    v.get(key).is_some_and(truthy)
}
fn defined(v: &Value, key: &str) -> bool {
    v.get(key).is_some()
}
fn number(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(Value::as_f64)
}
fn num(v: f64) -> Value {
    serde_json::Number::from_f64(v).map_or(Value::Null, Value::Number)
}
fn object() -> Value {
    Value::Object(Map::new())
}
fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("fixed upstream regex")
}
fn js_whitespace(ch: char) -> bool {
    matches!(ch, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' |
        '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
}
fn js_trim(value: &str) -> &str {
    value.trim_matches(js_whitespace)
}
fn effort_value(v: &Value) -> Option<&str> {
    v.as_str().filter(|v| EFFORTS.contains(v))
}
fn effort_list(v: &Value) -> Option<&Vec<Value>> {
    v.as_array().filter(|a| !a.is_empty() && a.iter().all(|v| effort_value(v).is_some()))
}
fn effort_record(v: Option<&Value>, numeric: bool) -> Option<Value> {
    let v = v?.as_object()?;
    let out: Map<String, Value> = v
        .iter()
        .filter(|(k, v)| EFFORTS.contains(&k.as_str()) && if numeric { v.is_number() } else { v.is_string() })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    (!out.is_empty()).then_some(Value::Object(out))
}
fn merge_object(target: &mut Value, source: &Value) {
    if let (Some(target), Some(source)) = (target.as_object_mut(), source.as_object()) {
        target.extend(source.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
}

struct IdentityFacts<'a> {
    identity: &'a Value,
    revision: Option<Revision>,
}
impl<'a> IdentityFacts<'a> {
    fn new(identity: &'a Value) -> Self {
        Self { identity, revision: identity.get("revision").and_then(Value::as_str).and_then(parse_revision) }
    }
    fn is(&self, class: &str) -> bool {
        string(self.identity, "class") == class
    }
    fn family(&self, families: &[&str]) -> bool {
        families.contains(&string(self.identity, "family"))
    }
    fn rev_gte(&self, bound: &str) -> bool {
        self.revision.zip(parse_revision(bound)).is_some_and(|(a, b)| compare_revision(a, b) >= 0)
    }
    fn major(&self) -> Option<u8> {
        self.revision.map(|v| v[0])
    }
    fn kimi_mandatory(&self) -> bool {
        self.is("kimi") && self.family(&["k2.7-code", "k3"])
    }
    fn adaptive_at_least(&self, opus_min: &str) -> bool {
        self.is("anthropic")
            && ((self.family(&["opus"]) && self.rev_gte(opus_min))
                || (self.family(&["sonnet", "fable", "mythos"]) && self.rev_gte("5")))
    }
}

/// Keep declared optional keys even when their value is undefined. The upstream
/// sparse override applicator accepts only keys present on the detected record.
#[derive(Clone)]
struct CompatRecord {
    values: Value,
    declared: BTreeSet<String>,
    prototype: CompatPrototype,
}
#[derive(Clone)]
enum CompatPrototype {
    ObjectDefault,
    Authored(Value),
    Null,
}
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
const ARRAY_PROTOTYPE_KEYS: &[&str] = &[
    "length",
    "constructor",
    "at",
    "concat",
    "copyWithin",
    "fill",
    "find",
    "findIndex",
    "findLast",
    "findLastIndex",
    "lastIndexOf",
    "pop",
    "push",
    "reverse",
    "shift",
    "unshift",
    "slice",
    "sort",
    "splice",
    "includes",
    "indexOf",
    "join",
    "keys",
    "entries",
    "values",
    "forEach",
    "filter",
    "flat",
    "flatMap",
    "map",
    "every",
    "some",
    "reduce",
    "reduceRight",
    "toReversed",
    "toSorted",
    "toSpliced",
    "with",
    "toLocaleString",
    "toString",
];
fn array_index(key: &str) -> Option<usize> {
    key.parse::<usize>().ok().filter(|v| *v < u32::MAX as usize && v.to_string() == key)
}
pub(crate) fn array_prototype_has_key(key: &str, len: usize) -> bool {
    ARRAY_PROTOTYPE_KEYS.contains(&key) || array_index(key).is_some_and(|index| index < len)
}
impl CompatPrototype {
    fn has(&self, key: &str) -> bool {
        match self {
            Self::Null => false,
            Self::ObjectDefault => OBJECT_PROTOTYPE_KEYS.contains(&key),
            Self::Authored(v) => {
                v.as_object().is_some_and(|m| m.contains_key(key))
                    || v.as_array().is_some_and(|a| {
                        ARRAY_PROTOTYPE_KEYS.contains(&key) || array_index(key).is_some_and(|i| i < a.len())
                    })
                    || OBJECT_PROTOTYPE_KEYS.contains(&key)
            }
        }
    }
    fn proto_is_accessor(&self) -> bool {
        !matches!(self, Self::Null)
            && !matches!(self, Self::Authored(v) if v.as_object().is_some_and(|m| m.contains_key("__proto__")))
    }
}
impl CompatRecord {
    fn new(values: Value, optional: &[&str]) -> Self {
        let mut declared = values.as_object().expect("compat object").keys().cloned().collect::<BTreeSet<_>>();
        declared.extend(optional.iter().map(|v| (*v).to_owned()));
        Self { values, declared, prototype: CompatPrototype::ObjectDefault }
    }
    fn set(&mut self, key: &str, value: Value) {
        self.declared.insert(key.to_owned());
        self.values.as_object_mut().expect("compat object").insert(key.to_owned(), value);
    }
    fn set_opt(&mut self, key: &str, value: Option<Value>) {
        self.declared.insert(key.to_owned());
        if let Some(value) = value {
            self.set(key, value);
        } else {
            self.values.as_object_mut().expect("compat object").remove(key);
        }
    }
    fn overrides(&mut self, overrides: Option<&Value>) {
        if let Some(overrides) = overrides.and_then(Value::as_object) {
            for (key, value) in ara_prompt::js::entries(overrides) {
                let own = self.declared.contains(key);
                if !own && !self.prototype.has(key) {
                    continue;
                }
                if key == "__proto__" && !own && self.prototype.proto_is_accessor() {
                    if value.is_null() {
                        self.prototype = CompatPrototype::Null;
                    } else if value.is_object() || value.is_array() {
                        self.prototype = CompatPrototype::Authored(value.clone());
                    }
                } else {
                    self.set(key, value.clone());
                }
            }
        }
    }
    fn effective_values(&self) -> Value {
        let mut out = self.values.clone();
        if let CompatPrototype::Authored(v) = &self.prototype {
            if let Some(v) = v.as_object() {
                for (key, value) in v {
                    if !self.declared.contains(key) {
                        out[key] = value.clone();
                    }
                }
            } else if let Some(v) = v.as_array() {
                for (i, value) in v.iter().enumerate() {
                    let key = i.to_string();
                    if !self.declared.contains(&key) {
                        out[&key] = value.clone();
                    }
                }
                if !self.declared.contains("length") {
                    out["length"] = json!(v.len());
                }
            }
        }
        out
    }
}

/// Sparse override application to a supplied JSON object. This preserves the
/// upstream `in` operator's Object.prototype membership and `__proto__` setter
/// order. Internal resolution additionally tracks declared undefined keys and
/// the non-serialized prototype across the full model construction.
pub fn apply_compat_overrides(compat: &mut Value, overrides: Option<&Value>) {
    if compat.is_object() {
        let mut record = CompatRecord::new(compat.clone(), &[]);
        record.overrides(overrides);
        *compat = record.values;
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Record {
    OpenAI,
    Responses,
    Anthropic,
    Bedrock,
    Devin,
    Google,
}
fn api_records(api: &str) -> &'static [Record] {
    use Record::*;
    match api {
        "openai-completions" => &[OpenAI],
        "openrouter" => &[OpenAI, Responses],
        "openai-responses" | "azure-openai-responses" | "openai-codex-responses" => &[Responses],
        "anthropic-messages" => &[Anthropic],
        "bedrock-converse-stream" => &[Bedrock],
        "devin-agent" => &[Devin],
        "google-generative-ai" | "google-vertex" | "google-gemini-cli" => &[Google],
        _ => &[],
    }
}
/// The complete closed AXES wire applicability table, grouped by record sets.
fn wire_records(key: &str) -> &'static [Record] {
    use Record::*;
    match key {
        "allowsSyntheticReasoningContentForToolCalls"
        | "alwaysSendMaxTokens"
        | "cacheControlFormat"
        | "disableReasoningOnForcedToolChoice"
        | "disableReasoningOnToolChoice"
        | "emptyLengthFinishIsContextError"
        | "filterReasoningHistory"
        | "includeEncryptedReasoning"
        | "omitReasoningEffort"
        | "promptCacheBreakpointTtl"
        | "promptCacheSessionHeader"
        | "rejectRootObjectUnion"
        | "retryWithoutStrictOnGrammarError"
        | "reasoningContentField"
        | "reasoningDeltasMayBeCumulative"
        | "reasoningDisableMode"
        | "reasoningEffortMap"
        | "requiresAssistantContentForToolCalls"
        | "requiresReasoningContentForAllAssistantTurns"
        | "requiresReasoningContentForToolCalls"
        | "stripDeepseekSpecialTokens"
        | "streamMarkupHealingPattern"
        | "supportsDeveloperRole"
        | "supportsNamedToolChoice"
        | "supportsPenaltyAndStopParams"
        | "supportsPromptCacheBreakpoints"
        | "supportsReasoningEffort"
        | "supportsReasoningParams"
        | "supportsStrictMode"
        | "supportsToolChoice"
        | "thinkingFormat"
        | "toolSchemaFlavor"
        | "usesOpenAIToolCallIdLimit"
        | "wireModelIdMode" => &[OpenAI, Responses],
        "clampOutputToModelMax"
        | "dropThinkingWhenReasoningEffort"
        | "extraBody"
        | "kimiApiFormat"
        | "maxTokensField"
        | "nativeKimiK3Reasoning"
        | "qwenPreserveThinking"
        | "replayReasoningContent"
        | "requiresAssistantAfterToolResult"
        | "requiresMistralToolIds"
        | "requiresThinkingAsText"
        | "requiresToolResultName"
        | "supportsMultipleSystemMessages"
        | "supportsPromptCacheKey"
        | "supportsStore"
        | "supportsUsageInStreaming"
        | "qwenTemplateReasoningEffort"
        | "thinkingKeep"
        | "toolStrictMode"
        | "whenThinking"
        | "zaiReasoningEffortDialect" => &[OpenAI],
        "strictResponsesPairing"
        | "requiresReasoningOffJuiceInstruction"
        | "supportsAllTurnsReasoningContext"
        | "supportsConfigurationUpdate"
        | "supportsImageDetailOriginal"
        | "supportsObfuscationOptOut"
        | "harmonyLeakMitigation"
        | "supportsReasoningSummary" => &[Responses],
        "allowAnthropicHeaderOverrides"
        | "disableAdaptiveThinking"
        | "disableStrictTools"
        | "escapeBuiltinToolNames"
        | "injectClaudeCodeInstruction"
        | "replayUnsignedThinking"
        | "requiresThinkingEnabled"
        | "requiresToolResultId"
        | "signingEndpoint"
        | "supportsContextManagement"
        | "supportsOutputEffort"
        | "supportsEagerToolInputStreaming"
        | "supportsLongCacheRetention"
        | "supportsMidConversationSystem"
        | "supportsMidConversationToolChanges"
        | "supportsPerMessageEffort"
        | "supportsThinkingBindingControls"
        | "supportsTurnScopedSystem" => &[Anthropic],
        "officialEndpoint" => &[Anthropic, Responses],
        "supportsLongPromptCacheRetention" => &[OpenAI, Responses, Bedrock],
        "promptCacheMaximumCheckpoints" | "promptCacheMinimumTokens" | "promptCacheMode" => &[Bedrock],
        "modelRouter" | "supportsParallelToolCalls" | "trustExplicitThinkingOnly" => &[Devin],
        "antigravityClaudeToolMode"
        | "antigravityUsageLabel"
        | "ccaLegacyParametersSchema"
        | "claudeThinkingBetaHeader"
        | "dropUnsignedThinking"
        | "flashStreamLeakWorkaround"
        | "multimodalFunctionResponse"
        | "requiresSkipThoughtSignature"
        | "requiresSkipThoughtSignatureOnFirstFunctionCall"
        | "supportsFunctionPartId" => &[Google],
        "streamFirstEventTimeoutMs" => &[OpenAI, Responses, Google],
        "streamIdleTimeoutMs" => &[OpenAI, Responses, Anthropic, Bedrock, Google],
        "stripImageInput" | "thinkingLoopGuard" => &[OpenAI, Responses, Anthropic, Google],
        "supportsForcedToolChoice" | "supportsSamplingParams" => &[OpenAI, Responses, Anthropic],
        _ => &[],
    }
}

const WIRE_AXIS_KEYS: &[&str] = &[
    "allowsSyntheticReasoningContentForToolCalls",
    "alwaysSendMaxTokens",
    "cacheControlFormat",
    "clampOutputToModelMax",
    "disableReasoningOnForcedToolChoice",
    "disableReasoningOnToolChoice",
    "dropThinkingWhenReasoningEffort",
    "emptyLengthFinishIsContextError",
    "extraBody",
    "filterReasoningHistory",
    "includeEncryptedReasoning",
    "kimiApiFormat",
    "maxTokensField",
    "nativeKimiK3Reasoning",
    "omitReasoningEffort",
    "promptCacheBreakpointTtl",
    "promptCacheSessionHeader",
    "qwenPreserveThinking",
    "rejectRootObjectUnion",
    "retryWithoutStrictOnGrammarError",
    "reasoningContentField",
    "reasoningDeltasMayBeCumulative",
    "reasoningDisableMode",
    "reasoningEffortMap",
    "replayReasoningContent",
    "requiresAssistantAfterToolResult",
    "requiresAssistantContentForToolCalls",
    "requiresMistralToolIds",
    "requiresReasoningContentForAllAssistantTurns",
    "requiresReasoningContentForToolCalls",
    "requiresThinkingAsText",
    "requiresToolResultName",
    "strictResponsesPairing",
    "requiresReasoningOffJuiceInstruction",
    "supportsAllTurnsReasoningContext",
    "supportsConfigurationUpdate",
    "stripDeepseekSpecialTokens",
    "streamMarkupHealingPattern",
    "supportsDeveloperRole",
    "supportsImageDetailOriginal",
    "supportsLongPromptCacheRetention",
    "supportsMultipleSystemMessages",
    "supportsNamedToolChoice",
    "supportsObfuscationOptOut",
    "harmonyLeakMitigation",
    "supportsPenaltyAndStopParams",
    "supportsPromptCacheBreakpoints",
    "supportsPromptCacheKey",
    "supportsReasoningEffort",
    "supportsReasoningParams",
    "supportsReasoningSummary",
    "supportsStore",
    "supportsStrictMode",
    "supportsToolChoice",
    "supportsUsageInStreaming",
    "qwenTemplateReasoningEffort",
    "thinkingFormat",
    "thinkingKeep",
    "toolSchemaFlavor",
    "toolStrictMode",
    "usesOpenAIToolCallIdLimit",
    "whenThinking",
    "wireModelIdMode",
    "zaiReasoningEffortDialect",
    "allowAnthropicHeaderOverrides",
    "disableAdaptiveThinking",
    "disableStrictTools",
    "escapeBuiltinToolNames",
    "injectClaudeCodeInstruction",
    "officialEndpoint",
    "replayUnsignedThinking",
    "requiresThinkingEnabled",
    "requiresToolResultId",
    "signingEndpoint",
    "supportsContextManagement",
    "supportsOutputEffort",
    "supportsEagerToolInputStreaming",
    "supportsLongCacheRetention",
    "supportsMidConversationSystem",
    "supportsMidConversationToolChanges",
    "supportsPerMessageEffort",
    "supportsThinkingBindingControls",
    "supportsTurnScopedSystem",
    "promptCacheMaximumCheckpoints",
    "promptCacheMinimumTokens",
    "promptCacheMode",
    "modelRouter",
    "supportsParallelToolCalls",
    "trustExplicitThinkingOnly",
    "antigravityClaudeToolMode",
    "antigravityUsageLabel",
    "ccaLegacyParametersSchema",
    "claudeThinkingBetaHeader",
    "dropUnsignedThinking",
    "flashStreamLeakWorkaround",
    "multimodalFunctionResponse",
    "requiresSkipThoughtSignature",
    "requiresSkipThoughtSignatureOnFirstFunctionCall",
    "supportsFunctionPartId",
    "streamFirstEventTimeoutMs",
    "streamIdleTimeoutMs",
    "stripImageInput",
    "supportsForcedToolChoice",
    "supportsSamplingParams",
    "thinkingLoopGuard",
];
fn record_name(record: Record) -> &'static str {
    match record {
        Record::OpenAI => "openai",
        Record::Responses => "openai-responses",
        Record::Anthropic => "anthropic",
        Record::Bedrock => "bedrock",
        Record::Devin => "devin",
        Record::Google => "google",
    }
}
fn wire_directive(key: &str) -> String {
    if key == "qwenTemplateReasoningEffort" {
        return "template-reasoning-effort".into();
    }
    let expanded = key.replace("OpenAI", "Openai");
    let mut out = String::new();
    for ch in expanded.chars() {
        if ch.is_ascii_uppercase() {
            out.push('-');
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}
fn wire_values(key: &str) -> Option<&'static [&'static str]> {
    match key {
        "cacheControlFormat" => Some(&["anthropic"]),
        "kimiApiFormat" => Some(&["openai", "anthropic"]),
        "maxTokensField" => Some(&["max_completion_tokens", "max_tokens"]),
        "promptCacheBreakpointTtl" => Some(&["30m"]),
        "promptCacheSessionHeader" => Some(&["x-grok-conv-id"]),
        "reasoningContentField" => Some(&["reasoning_content", "reasoning", "reasoning_text"]),
        "reasoningDisableMode" => Some(&[
            "omit",
            "lowest-effort",
            "none-effort",
            "openrouter-enabled-false",
            "cline-enabled-false",
            "venice-disable-thinking",
            "zai-thinking-disabled",
            "qwen-enable-thinking-false",
            "qwen-template-false",
            "chat-template-thinking-false",
        ]),
        "streamMarkupHealingPattern" => Some(&["kimi", "dsml", "qwen", "thinking", "harmony"]),
        "thinkingFormat" => {
            Some(&["openai", "openrouter", "zai", "kimi", "qwen", "qwen-chat-template", "chat-template"])
        }
        "toolSchemaFlavor" => Some(&["moonshot-mfjs", "grammar", "none"]),
        "toolStrictMode" => Some(&["all_strict", "none", "mixed"]),
        "wireModelIdMode" => Some(&["raw", "cline-pass", "firepass", "fireworks", "openrouter"]),
        "promptCacheMode" => Some(&["none", "automatic", "explicit"]),
        "thinkingLoopGuard" => Some(&["gemini", "deepseek", "xai"]),
        _ => None,
    }
}
/// Complete closed directive metadata, shared by native runtime applicability
/// and callers inspecting the fixed upstream compiler vocabulary.
pub fn axes_value() -> &'static Value {
    static AXES: OnceLock<Value> = OnceLock::new();
    AXES.get_or_init(|| {
        let mut out = object();
        for key in WIRE_AXIS_KEYS {
            let records = wire_records(key).iter().copied().map(record_name).collect::<Vec<_>>();
            let mut axis = json!({"key":key,"set":"wire","shape": if matches!(*key,"extraBody"|"reasoningEffortMap"|"whenThinking") { "object" } else { "scalar" },"records":records});
            if let Some(values) = wire_values(key) { axis["values"] = json!(values); }
            if *key == "extraBody" { axis["verbatimKeys"] = json!(true); }
            out[wire_directive(key)] = axis;
        }
        for (directive,key,shape,values) in [
            ("thinking-default-level","defaultLevel","scalar",Some(EFFORTS)),
            ("thinking-effort-budgets","effortBudgets","object",None),
            ("thinking-effort-map","effortMap","object",None),
            ("thinking-efforts","efforts","array",Some(EFFORTS)),
            ("thinking-mode","mode","scalar",Some(THINKING_MODES)),
            ("thinking-requires-effort","requiresEffort","scalar",None),
            ("thinking-prefix-binding","prefixBinding","scalar",None),
            ("thinking-suppress-when-off","suppressWhenOff","scalar",None),
            ("thinking-supports-display","supportsDisplay","scalar",None),
        ] {
            let mut axis = json!({"key":key,"set":"thinking","shape":shape});
            if let Some(values) = values { axis["values"] = json!(values); }
            out[directive] = axis;
        }
        for (directive,key,shape,values) in [
            ("apply-patch-tool-type","applyPatchToolType","scalar",Some(&["freeform","function"][..])),
            ("context-promotion-target","contextPromotionTarget","scalar",None),
            ("context-window-floor","contextWindowFloor","scalar",None),
            ("cost-patch","costPatch","object",None),
            ("edit-revision","editRevision","scalar",None),
            ("input-modalities","inputModalities","array",Some(&["text","image"][..])),
            ("limits-patch","limitsPatch","object",None),
            ("long-context-cost","longContext","object",None),
            ("long-usage-limit-fallback","longUsageLimitFallback","scalar",None),
            ("requires-cursor-tool-schema-projection","requiresCursorToolSchemaProjection","scalar",None),
            ("priority","priority","scalar",None),
            ("service-tier-cost","serviceTierCost","object",None),
        ] {
            let mut axis = json!({"key":key,"set":"catalog","shape":shape});
            if let Some(values) = values { axis["values"] = json!(values); }
            out[directive] = axis;
        }
        out
    })
}
pub fn api_compat_records_value() -> &'static Value {
    static RECORDS: OnceLock<Value> = OnceLock::new();
    RECORDS.get_or_init(|| {
        let mut out = object();
        for api in [
            "openai-completions",
            "openrouter",
            "openai-responses",
            "azure-openai-responses",
            "openai-codex-responses",
            "anthropic-messages",
            "bedrock-converse-stream",
            "devin-agent",
            "google-generative-ai",
            "google-vertex",
            "google-gemini-cli",
        ] {
            out[api] = json!(api_records(api).iter().copied().map(record_name).collect::<Vec<_>>());
        }
        out
    })
}
pub fn effort_tiers_value() -> &'static Value {
    static TIERS: OnceLock<Value> = OnceLock::new();
    TIERS.get_or_init(|| json!(["minimal", "low", "medium", "high", "xhigh", "max", "off"]))
}
pub fn is_effort_tier(value: &str) -> bool {
    value == "off" || EFFORTS.contains(&value)
}
pub fn is_thinking_mode(value: &str) -> bool {
    THINKING_MODES.contains(&value)
}
fn apply_wire_axes(compat: &mut CompatRecord, axes: &Value, api: &str) {
    if let Some(wire) = axes.get("wire").and_then(Value::as_object) {
        for (key, value) in wire {
            if wire_records(key).iter().any(|r| api_records(api).contains(r)) {
                compat.set(key, value.clone());
            }
        }
    }
}
fn overlay_effort_map(compat: &mut CompatRecord, axes: &Value, spec: &Value) {
    if let Some(mut axis) = effort_record(axes["wire"].get("reasoningEffortMap"), false)
        && let Some(overrides) = spec["compat"].get("reasoningEffortMap").filter(|v| truthy(v))
    {
        merge_object(&mut axis, overrides);
        compat.set("reasoningEffortMap", axis);
    }
}

pub fn is_official_anthropic_api_url(base_url: Option<&str>) -> bool {
    base_url.filter(|v| !v.is_empty()).is_none_or(|v| {
        let lower = v.to_lowercase();
        lower == "https://api.anthropic.com" || lower.starts_with("https://api.anthropic.com/")
    })
}
pub fn is_azure_anthropic_route(base_url: Option<&str>) -> bool {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    base_url.is_some_and(|v| {
        PATTERN
            .get_or_init(|| regex(r"(?i-u)(?:^|//|\.)[a-z0-9-]+\.(?:inference|services)\.ai\.azure\.com"))
            .is_match(v)
    })
}
pub fn is_anthropic_signing_proxy_url(base_url: Option<&str>) -> bool {
    let Some(v) = base_url else {
        return false;
    };
    static PATTERNS: OnceLock<[Regex; 3]> = OnceLock::new();
    host_matches_url(v, "githubCopilot")
        || host_matches_url(v, "zenmux")
        || PATTERNS
            .get_or_init(|| {
                [
                    regex(r"(?i-u:gateway\.ai\.cloudflare\.com)/[^\n\r\u2028\u2029]+/(?i-u:anthropic)(?:/|$)"),
                    regex(r"(?i-u:aiplatform\.googleapis\.com)/[^\n\r\u2028\u2029]+/(?i-u:publishers/anthropic)/"),
                    regex(r"(?i-u)(?:^|//|\.)bedrock-runtime\.[a-z0-9-]+\.amazonaws\.com"),
                ]
            })
            .iter()
            .any(|r| r.is_match(v))
        || is_azure_anthropic_route(base_url)
}
pub fn xai_responses_reasoning_effort_map(model_id: &str) -> Result<Value, CatalogPolicyError> {
    let identity = classify_model("xai", model_id, ClassifyOptions { observed_at_ms: None, lenient: true })
        .map_err(|e| CatalogPolicyError(e.to_string()))?;
    let facts = IdentityFacts::new(&identity);
    Ok(if facts.is("xai") && facts.family(&["grok"]) && facts.rev_gte("4.6") {
        json!({"minimal":"low"})
    } else {
        json!({"minimal":"low", "xhigh":"high", "max":"high"})
    })
}
fn has_local_loopback_base_url(base_url: &str) -> bool {
    static PRIVATE_172: OnceLock<Regex> = OnceLock::new();
    let Ok(url) = reqwest::Url::parse(base_url) else {
        return false;
    };
    let host = url.host_str().unwrap_or("").to_lowercase();
    matches!(host.as_str(), "localhost" | "127.0.0.1" | "0.0.0.0" | "::1" | "[::1]")
        || host.starts_with("10.")
        || host.starts_with("192.168.")
        || PRIVATE_172.get_or_init(|| regex(r"^172\.(1[6-9]|2[0-9]|3[01])\.")).is_match(&host)
        || host.ends_with(".local")
}
fn local_provider(provider: &str) -> bool {
    matches!(provider, "llama.cpp" | "lm-studio" | "vllm" | "ollama")
}
pub(crate) fn reasoning_disable_mode(format: &str) -> &'static str {
    match format {
        "openrouter" => "openrouter-enabled-false",
        "zai" | "kimi" => "zai-thinking-disabled",
        "qwen" => "qwen-enable-thinking-false",
        "qwen-chat-template" => "qwen-template-false",
        "chat-template" => "chat-template-thinking-false",
        _ => "lowest-effort",
    }
}
fn official_openai_endpoint(provider: &str, base_url: &str) -> bool {
    provider == "openai"
        && (base_url.is_empty() || reqwest::Url::parse(base_url).is_ok_and(|u| u.host_str() == Some("api.openai.com")))
}
fn detect_stream_markup_healing(provider: &str, facts: &IdentityFacts<'_>, base_url: &str) -> Option<Value> {
    let kimi_k2 = facts.is("kimi") && string(facts.identity, "family").starts_with("k2");
    let value = if provider == "kimi-code" || provider == "moonshot" || kimi_k2 {
        Some("kimi")
    } else if facts.is("deepseek")
        && matches!(
            provider,
            "ollama" | "ollama-cloud" | "nvidia" | "deepseek" | "fireworks" | "nanogpt" | "opencode-go" | "openrouter"
        )
    {
        Some("dsml")
    } else if official_openai_endpoint(provider, base_url) {
        None
    } else {
        Some("thinking")
    };
    value.map(|v| json!(v))
}

struct OpenAIDetection {
    cline_pass: bool,
    zai: bool,
    zhipu: bool,
    moonshot: bool,
    opencode_host: bool,
    opencode_provider: bool,
    direct_deepseek: bool,
    deepseek_reasoning: bool,
    direct_deepseek_reasoning: bool,
    venice: bool,
    requires_enabled_thinking: bool,
    xiaomi_mimo: bool,
    local_openai: bool,
    local_serving: bool,
    openrouter: bool,
}
fn detect_openai(spec: &Value, facts: &IdentityFacts<'_>) -> OpenAIDetection {
    let host = |host| model_matches_host(spec, host);
    let provider = string(spec, "provider");
    let moonshot = host("moonshotNative");
    let direct_deepseek = host("deepseekDirect");
    let deepseek_reasoning = (host("deepseekFamily") || facts.is("deepseek")) && flag(spec, "reasoning");
    let local_loopback = has_local_loopback_base_url(string(spec, "baseUrl"));
    let local_openai = provider != "litellm" && (local_provider(provider) || local_loopback);
    OpenAIDetection {
        cline_pass: provider == "cline-pass",
        zai: host("zai"),
        zhipu: host("zhipu"),
        moonshot,
        opencode_host: host("opencode"),
        opencode_provider: matches!(provider, "opencode-go" | "opencode-zen"),
        direct_deepseek,
        deepseek_reasoning,
        direct_deepseek_reasoning: direct_deepseek && deepseek_reasoning,
        venice: host("venice"),
        requires_enabled_thinking: moonshot && facts.is("kimi") && facts.family(&["k2.7-code"]),
        xiaomi_mimo: host("xiaomi") && facts.is("mimo"),
        local_openai,
        local_serving: local_openai || local_loopback,
        openrouter: host("openrouter"),
    }
}

fn detect_openai_compat(spec: &Value, facts: &IdentityFacts<'_>, d: &OpenAIDetection) -> CompatRecord {
    let provider = string(spec, "provider");
    let base_url = string(spec, "baseUrl");
    let host = |host| model_matches_host(spec, host);
    let url = |host| host_matches_url(base_url, host);
    let cerebras = host("cerebras");
    let cerebras_host = url("cerebras");
    let kilo = host("kilo");
    let alibaba = host("alibabaDashscope");
    let xiaomi = host("xiaomi");
    let grok = host("xai");
    let mistral = host("mistral");
    let google_ai = url("googleAistudio");
    let openai = host("openai");
    let azure = host("azureOpenAI");
    let vercel = host("vercelAIGateway");
    let together = host("together");
    let fireworks = url("fireworks");
    let groq = host("groq");
    let minimax = host("minimax");
    let qwen_portal = host("qwenPortal");
    let moonshot_kimi = facts.is("kimi") && d.moonshot;
    let moonshot_k3 = moonshot_kimi && facts.family(&["k3"]);
    let preserved_thinking = moonshot_kimi && facts.family(&["k2.6"]);
    let anthropic = host("anthropic") || facts.is("anthropic");
    let qwen = facts.is("qwen");
    let deepseek = host("deepseekFamily") || facts.is("deepseek");
    let zai_effort = (d.zai || d.zhipu) && facts.is("glm") && facts.rev_gte("5.2");
    let nonstandard = cerebras
        || grok
        || mistral
        || google_ai
        || url("chutes")
        || url("deepseekFamily")
        || fireworks
        || alibaba
        || d.zai
        || d.zhipu
        || kilo
        || qwen
        || xiaomi
        || d.moonshot
        || d.opencode_host;
    let max_tokens = mistral || d.moonshot || d.zai || d.zhipu || url("chutes") || fireworks || d.direct_deepseek;
    let breakpoints = official_openai_endpoint(provider, base_url) && facts.is("openai") && facts.rev_gte("5.6");
    let multiple_system = !minimax
        && !alibaba
        && !qwen_portal
        && !qwen
        && (openai
            || azure
            || d.openrouter
            || cerebras
            || together
            || fireworks
            || groq
            || deepseek
            || mistral
            || grok
            || d.zai
            || d.zhipu
            || matches!(provider, "github-copilot" | "zenmux"));
    let idle = if facts.is("glm")
        && facts.rev_gte("5")
        && !facts.family(&["vision"])
        && (url("zai") || url("zhipu") || url("opencode"))
    {
        Some(600_000)
    } else if (facts.is("mimo") && url("xiaomi"))
        || (flag(spec, "reasoning") && facts.is("kimi") && facts.family(&["k3", "k2.7-code"]) && url("moonshotNative"))
        || (flag(spec, "reasoning") && facts.is("deepseek") && url("deepseekDirect"))
        || d.local_serving
    {
        Some(300_000)
    } else {
        None
    };
    let thinking_format = if (facts.is("kimi") && !facts.family(&["k3"]) && url("moonshotNative"))
        || url("zai")
        || url("zhipu")
        || (facts.is("mimo") && url("xiaomi"))
    {
        "zai"
    } else if url("openrouter") {
        "openrouter"
    } else if qwen && url("nvidia") {
        "qwen-chat-template"
    } else if qwen && (fireworks || url("venice")) {
        "openai"
    } else if url("alibabaDashscope") || qwen {
        "qwen"
    } else {
        "openai"
    };
    let qwen_format = matches!(thinking_format, "qwen" | "qwen-chat-template");
    let mut c = CompatRecord::new(
        json!({
            "supportsStore": !nonstandard, "supportsDeveloperRole": openai || azure,
            "supportsMultipleSystemMessages": multiple_system,
            "supportsReasoningEffort": !grok && !d.xiaomi_mimo && (!(d.zai || d.zhipu) || zai_effort),
            "supportsReasoningParams": provider != "github-copilot",
            "supportsSamplingParams": !(facts.is("openai") && (facts.family(&["o-series"]) || facts.rev_gte("5"))),
            "supportsPenaltyAndStopParams": !(grok && flag(spec,"reasoning")),
            "reasoningEffortMap": {}, "supportsUsageInStreaming": !cerebras_host,
            "alwaysSendMaxTokens": facts.is("kimi"),
            "disableReasoningOnForcedToolChoice": !d.cline_pass && ((facts.is("kimi") && !moonshot_k3) || anthropic),
            "disableReasoningOnToolChoice": !d.cline_pass && deepseek && flag(spec,"reasoning") && !d.openrouter,
            "supportsToolChoice": d.cline_pass || !d.direct_deepseek_reasoning,
            "supportsForcedToolChoice": !d.requires_enabled_thinking && !(d.opencode_host && d.deepseek_reasoning) && !(d.cline_pass && qwen),
            "supportsNamedToolChoice": true, "maxTokensField": if max_tokens {"max_tokens"} else {"max_completion_tokens"},
            "requiresToolResultName": mistral, "requiresAssistantAfterToolResult": mistral,
            "requiresThinkingAsText": mistral, "requiresMistralToolIds": mistral, "thinkingFormat": thinking_format,
            "reasoningDisableMode": if d.cline_pass { "cline-enabled-false" } else if d.venice { "venice-disable-thinking" } else { reasoning_disable_mode(thinking_format) },
            "omitReasoningEffort": false, "includeEncryptedReasoning": true, "filterReasoningHistory": d.openrouter && anthropic,
            "reasoningContentField": if d.cline_pass { "reasoning" } else { "reasoning_content" },
            "requiresReasoningContentForToolCalls": (facts.is("kimi") && !d.opencode_provider) ||
                (deepseek && flag(spec,"reasoning")) || d.xiaomi_mimo || (d.openrouter && flag(spec,"reasoning")),
            "requiresReasoningContentForAllAssistantTurns": ((deepseek && flag(spec,"reasoning")) || d.xiaomi_mimo) && !d.openrouter,
            "allowsSyntheticReasoningContentForToolCalls": (!deepseek || !flag(spec,"reasoning")) && !d.xiaomi_mimo,
            "replayReasoningContent": d.local_openai, "qwenPreserveThinking": qwen_format && d.local_openai,
            "qwenTemplateReasoningEffort": qwen_format && d.local_openai && provider != "ollama" && qwen && facts.rev_gte("3.8"),
            "requiresAssistantContentForToolCalls": facts.is("kimi") || d.direct_deepseek_reasoning,
            "supportsPromptCacheBreakpoints": breakpoints, "isOpenRouterHost": d.openrouter,
            "wireModelIdMode": if url("openrouter") { "openrouter" } else { "raw" }, "isVercelGatewayHost": vercel,
            "supportsStrictMode": url("openai") || url("azureOpenAI") || url("cerebras") || url("together") || url("openrouter") || url("deepseekFamily"),
            "toolStrictMode": if cerebras_host { "all_strict" } else { "mixed" },
            "stripDeepseekSpecialTokens": facts.is("deepseek") && matches!(provider,"nvidia" | "deepseek"),
            "reasoningDeltasMayBeCumulative": false, "emptyLengthFinishIsContextError": false,
            "usesOpenAIToolCallIdLimit": false, "dropThinkingWhenReasoningEffort": false,
            "nativeKimiK3Reasoning": false, "zaiReasoningEffortDialect": false,
            "clampOutputToModelMax": false, "stripImageInput": false, "rejectRootObjectUnion": false,
            "retryWithoutStrictOnGrammarError": false, "supportsPromptCacheKey": false
        }),
        &[
            "kimiApiFormat",
            "thinkingKeep",
            "cacheControlFormat",
            "promptCacheBreakpointTtl",
            "openRouterRouting",
            "vercelGatewayRouting",
            "extraBody",
            "toolSchemaFlavor",
            "streamFirstEventTimeoutMs",
            "streamIdleTimeoutMs",
            "streamMarkupHealingPattern",
            "promptCacheSessionHeader",
            "thinkingLoopGuard",
        ],
    );
    if preserved_thinking {
        c.set("thinkingKeep", json!("all"));
    }
    if (d.cline_pass && (qwen || anthropic)) || (d.openrouter && anthropic) {
        c.set("cacheControlFormat", json!("anthropic"));
    }
    if breakpoints {
        c.set("promptCacheBreakpointTtl", json!("30m"));
    }
    if d.moonshot || facts.is("kimi") {
        c.set("toolSchemaFlavor", json!("moonshot-mfjs"));
    } else if d.local_openai {
        c.set("toolSchemaFlavor", json!("grammar"));
    }
    if d.local_serving {
        c.set("streamFirstEventTimeoutMs", json!(0));
    }
    c.set_opt("streamIdleTimeoutMs", idle.map(|v| json!(v)));
    c.set_opt("streamMarkupHealingPattern", detect_stream_markup_healing(provider, facts, base_url));
    if url("xai") {
        c.set("promptCacheSessionHeader", json!("x-grok-conv-id"));
    }
    c
}

fn fixup_openai_compat(spec: &Value, compat: &mut CompatRecord, d: &OpenAIDetection, axes: &Value) {
    if d.direct_deepseek_reasoning
        && compat.values["extraBody"]["thinking"].is_object()
        && string(&compat.values["extraBody"]["thinking"], "type") == "enabled"
    {
        let mut extra = compat.values["extraBody"].clone();
        extra.as_object_mut().expect("extra body object").remove("thinking");
        compat.set_opt("extraBody", (!extra.as_object().expect("extra body").is_empty()).then_some(extra));
    }
    if !defined(&spec["compat"], "reasoningDisableMode") && !defined(&axes["wire"], "reasoningDisableMode") {
        compat.set(
            "reasoningDisableMode",
            json!(if d.cline_pass {
                "cline-enabled-false"
            } else if d.requires_enabled_thinking {
                "omit"
            } else if d.direct_deepseek_reasoning {
                "zai-thinking-disabled"
            } else if d.venice {
                "venice-disable-thinking"
            } else {
                reasoning_disable_mode(string(&compat.values, "thinkingFormat"))
            }),
        );
    }
    if !defined(&spec["compat"], "omitReasoningEffort")
        && !defined(&axes["wire"], "omitReasoningEffort")
        && !flag(&compat.values, "supportsReasoningEffort")
    {
        compat.set("omitReasoningEffort", json!(true));
    }
    let axis_when = if flag(spec, "reasoning") {
        axes["wire"].get("whenThinking").filter(|v| v.is_object()).cloned()
    } else {
        None
    };
    let authored = spec["compat"].get("whenThinking").filter(|v| !v.is_null()).cloned();
    let when = authored.or(axis_when).or_else(|| {
        if !d.direct_deepseek_reasoning {
            return None;
        }
        let mut extra = object();
        merge_object(&mut extra, &compat.values["extraBody"]);
        extra["thinking"] = json!({"type":"enabled"});
        Some(json!({"extraBody":extra}))
    });
    if let Some(when) = when.filter(truthy) {
        let mut variant = compat.clone();
        // Object spread creates a fresh ordinary object, independently of the
        // detected record's prototype (including an authored null prototype).
        variant.prototype = CompatPrototype::ObjectDefault;
        variant.set_opt("whenThinking", None);
        variant.overrides(Some(&when));
        if !defined(&when, "reasoningDisableMode") {
            variant.set(
                "reasoningDisableMode",
                json!(if d.venice {
                    "venice-disable-thinking"
                } else {
                    reasoning_disable_mode(string(&variant.values, "thinkingFormat"))
                }),
            );
        }
        if !defined(&when, "omitReasoningEffort") && !flag(&variant.values, "supportsReasoningEffort") {
            variant.set("omitReasoningEffort", json!(true));
        }
        compat.set("whenThinking", variant.values);
    } else {
        compat.set_opt("whenThinking", None);
    }
}
fn resolve_completions(spec: &Value, facts: &IdentityFacts<'_>, axes: &Value) -> CompatRecord {
    let detection = detect_openai(spec, facts);
    let mut compat = detect_openai_compat(spec, facts, &detection);
    apply_wire_axes(&mut compat, axes, "openai-completions");
    compat.overrides(spec.get("compat"));
    overlay_effort_map(&mut compat, axes, spec);
    fixup_openai_compat(spec, &mut compat, &detection, axes);
    compat
}

fn resolve_responses(spec: &Value, facts: &IdentityFacts<'_>, axes: &Value, api: &str) -> CompatRecord {
    resolve_responses_stage(spec, facts, axes, api, true)
}
fn resolve_responses_stage(
    spec: &Value,
    facts: &IdentityFacts<'_>,
    axes: &Value,
    api: &str,
    apply_authored: bool,
) -> CompatRecord {
    let base_url = string(spec, "baseUrl");
    let provider = string(spec, "provider");
    let host = |host| model_matches_host(spec, host);
    let url = |host| host_matches_url(base_url, host);
    let azure = host("azureOpenAI");
    let openrouter = host("openrouter");
    let openai_url = url("openai");
    let vercel = host("vercelAIGateway");
    let xai = host("xai");
    let breakpoints = official_openai_endpoint(provider, base_url) && facts.is("openai") && facts.rev_gte("5.6");
    let thinking_format = if openrouter { "openrouter" } else { "openai" };
    let reasoning = flag(spec, "reasoning");
    let local_serving = (provider != "litellm" && local_provider(provider)) || has_local_loopback_base_url(base_url);
    let anthropic = facts.is("anthropic");
    let deepseek = facts.is("deepseek");
    let mut c = CompatRecord::new(
        json!({
            "supportsDeveloperRole": azure || openai_url || url("githubCopilot"),
            "supportsStrictMode": azure || url("openai") || url("azureOpenAI") || url("cerebras") || url("together") || url("openrouter") || url("deepseekFamily"),
            "supportsReasoningEffort": !xai, "supportsLongPromptCacheRetention": openai_url,
            "supportsPromptCacheBreakpoints": breakpoints, "strictResponsesPairing": azure || provider == "github-copilot",
            "supportsImageDetailOriginal": !xai && !host("githubCopilot"), "supportsReasoningSummary": !xai,
            "supportsAllTurnsReasoningContext": false, "supportsConfigurationUpdate": false,
            "requiresReasoningOffJuiceInstruction": false, "stripImageInput": false,
            "reasoningEffortMap": {}, "supportsReasoningParams": true,
            "supportsSamplingParams": !(facts.is("openai") && (facts.family(&["o-series"]) || facts.rev_gte("5"))),
            "supportsPenaltyAndStopParams": !xai, "thinkingFormat": thinking_format,
            "reasoningDisableMode": reasoning_disable_mode(thinking_format), "omitReasoningEffort": false,
            "includeEncryptedReasoning": true, "filterReasoningHistory": openrouter && anthropic,
            "disableReasoningOnForcedToolChoice": facts.is("kimi"), "disableReasoningOnToolChoice": deepseek && reasoning && !openrouter,
            "supportsToolChoice": true, "supportsForcedToolChoice": !matches!(provider,"opencode-go" | "opencode-zen"),
            "supportsNamedToolChoice": true, "reasoningContentField": "reasoning_content",
            "requiresReasoningContentForToolCalls": (facts.is("kimi") || (deepseek && reasoning) || (openrouter && reasoning)) && reasoning,
            "requiresReasoningContentForAllAssistantTurns": deepseek && reasoning && !openrouter,
            "allowsSyntheticReasoningContentForToolCalls": !deepseek || !reasoning, "replayReasoningContent": false,
            "qwenPreserveThinking": false, "qwenTemplateReasoningEffort": false, "requiresThinkingAsText": false,
            "requiresMistralToolIds": false, "requiresToolResultName": false, "requiresAssistantAfterToolResult": false,
            "requiresAssistantContentForToolCalls": facts.is("kimi"), "isOpenRouterHost": openrouter,
            "isVercelGatewayHost": vercel, "wireModelIdMode": if openrouter { "openrouter" } else { "raw" },
            "alwaysSendMaxTokens": facts.is("kimi"), "supportsObfuscationOptOut": openai_url || provider == "openai",
            "officialEndpoint": official_openai_endpoint(provider,base_url), "harmonyLeakMitigation": false,
            "rejectRootObjectUnion": false, "retryWithoutStrictOnGrammarError": false,
            "stripDeepseekSpecialTokens": facts.is("deepseek") && matches!(provider,"nvidia" | "deepseek"),
            "reasoningDeltasMayBeCumulative": false, "emptyLengthFinishIsContextError": false, "usesOpenAIToolCallIdLimit": false
        }),
        &[
            "thinkingLoopGuard",
            "promptCacheBreakpointTtl",
            "openRouterRouting",
            "vercelGatewayRouting",
            "toolSchemaFlavor",
            "cacheControlFormat",
            "streamMarkupHealingPattern",
            "promptCacheSessionHeader",
            "streamFirstEventTimeoutMs",
            "streamIdleTimeoutMs",
        ],
    );
    if breakpoints {
        c.set("promptCacheBreakpointTtl", json!("30m"));
    }
    if facts.is("kimi") {
        c.set("toolSchemaFlavor", json!("moonshot-mfjs"));
    }
    if openrouter && anthropic {
        c.set("cacheControlFormat", json!("anthropic"));
    }
    c.set_opt("streamMarkupHealingPattern", detect_stream_markup_healing(provider, facts, base_url));
    if url("xai") {
        c.set("promptCacheSessionHeader", json!("x-grok-conv-id"));
    }
    c.set_opt(
        "streamFirstEventTimeoutMs",
        if local_serving { Some(json!(0)) } else { spec["compat"].get("streamFirstEventTimeoutMs").cloned() },
    );
    c.set_opt(
        "streamIdleTimeoutMs",
        if local_serving { Some(json!(300_000)) } else { spec["compat"].get("streamIdleTimeoutMs").cloned() },
    );
    apply_wire_axes(&mut c, axes, api);
    if !apply_authored {
        return c;
    }
    c.overrides(spec.get("compat"));
    overlay_effort_map(&mut c, axes, spec);
    if xai && defined(&axes["wire"], "reasoningEffortMap") {
        let canonical = effort_record(axes["wire"].get("reasoningEffortMap"), false).unwrap_or_else(object);
        let mut mapped = object();
        merge_object(&mut mapped, &c.values["reasoningEffortMap"]);
        merge_object(&mut mapped, &canonical);
        for key in ["xhigh", "max"] {
            if !defined(&canonical, key) {
                mapped.as_object_mut().expect("map").remove(key);
            }
        }
        c.set("reasoningEffortMap", mapped);
    }
    if !defined(&spec["compat"], "reasoningDisableMode") && !defined(&axes["wire"], "reasoningDisableMode") {
        c.set("reasoningDisableMode", json!(reasoning_disable_mode(string(&c.values, "thinkingFormat"))));
    }
    if !defined(&spec["compat"], "omitReasoningEffort")
        && !defined(&axes["wire"], "omitReasoningEffort")
        && !flag(&c.values, "supportsReasoningEffort")
    {
        c.set("omitReasoningEffort", json!(true));
    }
    if provider == "xai-oauth"
        && axes["wire"]["supportsReasoningEffort"] == true
        && spec["compat"]["supportsReasoningEffort"] != false
    {
        c.set("supportsReasoningEffort", json!(true));
        c.set("omitReasoningEffort", json!(false));
    }
    c
}

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

fn resolve_anthropic(spec: &Value, facts: &IdentityFacts<'_>, axes: &Value) -> CompatRecord {
    let base_url = spec.get("baseUrl").and_then(Value::as_str);
    let official = is_official_anthropic_api_url(base_url);
    let requires_thinking = model_matches_host(spec, "moonshotNative") && facts.kimi_mandatory();
    let signing = official
        || model_matches_host(spec, "githubCopilot")
        || model_matches_host(spec, "zenmux")
        || is_anthropic_signing_proxy_url(base_url);
    let mut c = CompatRecord::new(
        json!({
            "officialEndpoint": official, "signingEndpoint": signing, "supportsContextManagement": true,
            "supportsOutputEffort": true, "disableStrictTools": is_azure_anthropic_route(base_url),
            "disableAdaptiveThinking": false, "allowAnthropicHeaderOverrides": false,
            "supportsEagerToolInputStreaming": official, "supportsLongCacheRetention": official,
            "supportsMidConversationSystem": official && !facts.family(&["sonnet"]) && facts.adaptive_at_least("4.8"),
            "supportsTurnScopedSystem": false, "supportsMidConversationToolChanges": false, "supportsPerMessageEffort": false,
            "supportsThinkingBindingControls": false,
            "supportsForcedToolChoice": !requires_thinking && !facts.family(&["fable","mythos"]),
            "supportsSamplingParams": !facts.adaptive_at_least("4.7"), "requiresToolResultId": false,
            "requiresThinkingEnabled": requires_thinking,
            "replayUnsignedThinking": !signing && (flag(spec,"reasoning") || model_matches_host(spec,"deepseekFamily")),
            "escapeBuiltinToolNames": false, "injectClaudeCodeInstruction": true, "stripImageInput": false
        }),
        &["thinkingLoopGuard", "streamIdleTimeoutMs"],
    );
    c.set_opt("streamIdleTimeoutMs", spec["compat"].get("streamIdleTimeoutMs").cloned());
    apply_wire_axes(&mut c, axes, "anthropic-messages");
    c.overrides(spec.get("compat"));
    c
}
fn resolve_simple(spec: &Value, axes: &Value, api: &str) -> CompatRecord {
    let values = match api {
        "bedrock-converse-stream" => json!({"promptCacheMode":"none", "supportsLongPromptCacheRetention":false,
            "promptCacheMinimumTokens":0,"promptCacheMaximumCheckpoints":0}),
        "devin-agent" => {
            json!({"trustExplicitThinkingOnly":true,"modelRouter":false,"supportsParallelToolCalls":false})
        }
        _ => json!({"supportsFunctionPartId":false,"requiresSkipThoughtSignature":false,
            "requiresSkipThoughtSignatureOnFirstFunctionCall":false,"dropUnsignedThinking":false,
            "ccaLegacyParametersSchema":false,"multimodalFunctionResponse":false,
            "flashStreamLeakWorkaround":false,"claudeThinkingBetaHeader":false,"antigravityClaudeToolMode":false,"stripImageInput":false}),
    };
    let mut c = CompatRecord::new(values, &[]);
    if api == "bedrock-converse-stream" {
        c.set_opt("streamIdleTimeoutMs", flag(spec, "reasoning").then(|| json!(600_000)));
    }
    apply_wire_axes(&mut c, axes, api);
    c.overrides(spec.get("compat"));
    c
}

fn default_thinking_mode(spec: &Value, facts: &IdentityFacts<'_>) -> &'static str {
    match string(spec, "api") {
        "google-generative-ai" | "google-gemini-cli" | "google-vertex" => {
            if facts.is("gemini") && facts.major() == Some(3) { "google-level" } else { "budget" }
        }
        "anthropic-messages" => {
            if facts.is("minimax") && facts.family(&["m2", "m3"]) {
                return "anthropic-adaptive";
            }
            if facts.is("glm") && facts.rev_gte("5.2") && matches!(string(spec, "provider"), "umans" | "zai") {
                return "anthropic-budget-effort";
            }
            if facts.is("anthropic") {
                if facts.rev_gte("4.6") && !facts.family(&["haiku"]) {
                    return "anthropic-adaptive";
                }
                if facts.family(&["opus"]) && facts.rev_gte("4.5") {
                    return "anthropic-budget-effort";
                }
            }
            "budget"
        }
        "bedrock-converse-stream" => {
            if facts.is("anthropic") {
                if facts.adaptive_at_least("4.6") {
                    return "anthropic-adaptive";
                }
                if facts.family(&["opus"]) && facts.rev_gte("4.5") {
                    return "anthropic-budget-effort";
                }
            }
            if facts.is("openai") { "effort" } else { "budget" }
        }
        _ => "effort",
    }
}
fn fallback_efforts(spec: &Value, compat: Option<&Value>) -> Value {
    let api = string(spec, "api");
    let xhigh = api == "anthropic-messages"
        || matches!(api, "openai-responses" | "openai-codex-responses" | "azure-openai-responses")
        || (matches!(api, "openai-completions" | "openrouter")
            && compat.is_some_and(|c| string(c, "thinkingFormat") == "openai" && flag(c, "supportsReasoningEffort")));
    json!(if xhigh { DEFAULT_EFFORTS_XHIGH } else { DEFAULT_EFFORTS })
}
fn read_rule_thinking(axes: &Value) -> Value {
    let raw = &axes["thinking"];
    let mut rule = object();
    if raw.get("mode").and_then(Value::as_str).is_some_and(|v| THINKING_MODES.contains(&v)) {
        rule["mode"] = raw["mode"].clone();
    }
    if let Some(v) = raw.get("efforts").and_then(effort_list) {
        rule["efforts"] = json!(v);
    }
    if raw.get("defaultLevel").and_then(effort_value).is_some() {
        rule["defaultLevel"] = raw["defaultLevel"].clone();
    }
    if let Some(v) = effort_record(raw.get("effortMap"), false) {
        rule["effortMap"] = v;
    }
    if let Some(v) = effort_record(raw.get("effortBudgets"), true) {
        rule["effortBudgets"] = v;
    }
    for key in ["requiresEffort", "suppressWhenOff", "supportsDisplay", "prefixBinding"] {
        if raw.get(key).is_some_and(Value::is_boolean) {
            rule[key] = raw[key].clone();
        }
    }
    rule
}
fn mandatory_reasoning(facts: &IdentityFacts<'_>, model_id: &str) -> bool {
    flag(facts.identity, "thinkingVariant") || strip_thinking_variant_suffix_utf16(model_id).is_some()
}
fn default_supports_display(spec: &Value, facts: &IdentityFacts<'_>) -> bool {
    matches!(string(spec, "api"), "anthropic-messages" | "bedrock-converse-stream") && facts.adaptive_at_least("4.7")
}
fn merged_effort_map(spec: &Value, rule_map: Option<&Value>, compat: Option<&Value>, efforts: &Value) -> Option<Value> {
    let detected =
        matches!(string(spec, "api"), "openai-completions" | "openrouter") && model_matches_host(spec, "fireworks");
    let configured =
        compat.and_then(|c| c.get("reasoningEffortMap")).filter(|v| v.as_object().is_some_and(|m| !m.is_empty()));
    if !detected && rule_map.is_none() && configured.is_none() {
        return None;
    }
    let mut map = if detected { json!({"minimal":"none"}) } else { object() };
    if let Some(v) = rule_map {
        merge_object(&mut map, v);
    }
    if let Some(v) = configured {
        merge_object(&mut map, v);
    }
    let mut filtered = Map::new();
    for effort in efforts.as_array().into_iter().flatten() {
        if let Some(effort) = effort.as_str()
            && let Some(v) = map.get(effort)
        {
            filtered.insert(effort.to_owned(), v.clone());
        }
    }
    (!filtered.is_empty()).then_some(Value::Object(filtered))
}
fn resolve_thinking(
    spec: &Value,
    facts: &IdentityFacts<'_>,
    axes: &Value,
    compat: Option<&Value>,
) -> Result<Option<Value>, CatalogPolicyError> {
    if !flag(spec, "reasoning") {
        return Ok(None);
    }
    let api = string(spec, "api");
    if (string(spec, "provider") == "cline-pass"
        || matches!(api, "openai-responses" | "openai-codex-responses" | "azure-openai-responses"))
        && compat.is_some_and(|c| c.get("supportsReasoningEffort") == Some(&Value::Bool(false)))
    {
        return Ok(None);
    }
    let rule = read_rule_thinking(axes);
    let supports_display =
        rule.get("supportsDisplay").and_then(Value::as_bool).unwrap_or_else(|| default_supports_display(spec, facts));
    let requires_effort = rule.get("requiresEffort").and_then(Value::as_bool).unwrap_or_else(|| {
        mandatory_reasoning(facts, string(spec, "id"))
            || compat.is_some_and(|c| c.get("qwenTemplateReasoningEffort") == Some(&Value::Bool(true)))
    });
    if let Some(explicit) = spec
        .get("thinking")
        .filter(|v| truthy(v) && v.get("efforts").and_then(Value::as_array).is_some_and(|a| !a.is_empty()))
    {
        let mut thinking = explicit.clone();
        if !defined(explicit, "effortMap")
            && let Some(map) = merged_effort_map(spec, rule.get("effortMap"), compat, &explicit["efforts"])
        {
            thinking["effortMap"] = map;
        }
        if !defined(explicit, "supportsDisplay") && supports_display {
            thinking["supportsDisplay"] = json!(true);
        }
        if !defined(explicit, "requiresEffort") && requires_effort {
            thinking["requiresEffort"] = json!(true);
        }
        if !defined(explicit, "defaultLevel")
            && let Some(v) = rule.get("defaultLevel")
        {
            thinking["defaultLevel"] = v.clone();
        }
        if !defined(explicit, "prefixBinding") && rule["prefixBinding"] == true {
            thinking["prefixBinding"] = json!(true);
        }
        return Ok(Some(thinking));
    }
    if compat.is_some_and(|c| c.get("trustExplicitThinkingOnly") == Some(&Value::Bool(true))) {
        return Ok(None);
    }
    let mut config = json!({
        "mode": rule.get("mode").cloned().unwrap_or_else(|| json!(default_thinking_mode(spec,facts))),
        "efforts": rule.get("efforts").cloned().unwrap_or_else(|| fallback_efforts(spec,compat))
    });
    if config["efforts"].as_array().is_none_or(Vec::is_empty) {
        return Err(CatalogPolicyError(format!(
            "Model {}/{} resolved to an empty thinking range",
            string(spec, "provider"),
            string(spec, "id")
        )));
    }
    if let Some(v) = rule.get("defaultLevel") {
        config["defaultLevel"] = v.clone();
    }
    if let Some(v) = merged_effort_map(spec, rule.get("effortMap"), compat, &config["efforts"]) {
        config["effortMap"] = v;
    }
    if let Some(v) = rule.get("effortBudgets") {
        config["effortBudgets"] = v.clone();
    }
    if supports_display {
        config["supportsDisplay"] = json!(true);
    }
    if flag(&rule, "prefixBinding") {
        config["prefixBinding"] = json!(true);
    }
    if requires_effort {
        config["requiresEffort"] = json!(true);
    }
    if flag(&rule, "suppressWhenOff") {
        config["suppressWhenOff"] = json!(true);
    }
    Ok(Some(config))
}

/// Resolve identity, every applicable compat key, thinking and catalog axes.
pub fn resolve_model_policy(spec: &Value) -> Result<Value, CatalogPolicyError> {
    if !spec.is_object() {
        return Err(CatalogPolicyError("Model spec must be an object".into()));
    }
    let identity = classify_model(
        string(spec, "provider"),
        string(spec, "id"),
        ClassifyOptions { observed_at_ms: None, lenient: false },
    )
    .map_err(|e| CatalogPolicyError(e.to_string()))?;
    let facts = IdentityFacts::new(&identity);
    let mut target = json!({"provider":string(spec,"provider"),"class":identity["class"],"model":string(spec,"id"),"reasoning":flag(spec,"reasoning")});
    for key in ["family", "revision"] {
        if let Some(v) = identity.get(key) {
            target[key] = v.clone();
        }
    }
    let axes = resolve_cascade(&target).map_err(|e| CatalogPolicyError(e.to_string()))?;
    let api = string(spec, "api");
    let compat = match api {
        "openrouter" => {
            let mut chat = resolve_completions(spec, &facts, &axes);
            let responses = resolve_responses(spec, &facts, &axes, "openrouter");
            for key in RESPONSES_ONLY {
                chat.set_opt(key, responses.values.get(*key).cloned());
            }
            chat.prototype = CompatPrototype::ObjectDefault;
            Some(chat)
        }
        "openai-completions" => Some(resolve_completions(spec, &facts, &axes)),
        "openai-responses" | "azure-openai-responses" | "openai-codex-responses" => {
            Some(resolve_responses(spec, &facts, &axes, api))
        }
        "anthropic-messages" => Some(resolve_anthropic(spec, &facts, &axes)),
        "bedrock-converse-stream" | "devin-agent" | "google-generative-ai" | "google-vertex" | "google-gemini-cli" => {
            Some(resolve_simple(spec, &axes, api))
        }
        _ => None,
    };
    let effective = compat.as_ref().map(CompatRecord::effective_values);
    let thinking = resolve_thinking(spec, &facts, &axes, effective.as_ref())?;
    let mut policy = json!({"identity":identity,"catalog":axes.get("catalog").cloned().unwrap_or_else(object)});
    if let Some(v) = compat {
        policy["compat"] = v.values;
    }
    if let Some(v) = thinking {
        policy["thinking"] = v;
    }
    Ok(policy)
}

/// Detector and compiled-axis allocation, before authored values or fixups.
/// The lossless Host applies the remaining source stages to its native record.
pub(crate) fn compat_seed_with_axes(spec: &Value, identity: &Value, axes: &Value, api: &str) -> Option<Value> {
    let facts = IdentityFacts::new(identity);
    let record = match api {
        "openai-completions" => {
            let detection = detect_openai(spec, &facts);
            let mut record = detect_openai_compat(spec, &facts, &detection);
            apply_wire_axes(&mut record, axes, api);
            record
        }
        "openrouter" | "openai-responses" | "azure-openai-responses" | "openai-codex-responses" => {
            resolve_responses_stage(spec, &facts, axes, api, false)
        }
        "anthropic-messages" => {
            let mut detector = spec.clone();
            detector.as_object_mut()?.remove("compat");
            resolve_anthropic(&detector, &facts, axes)
        }
        "bedrock-converse-stream" | "devin-agent" | "google-generative-ai" | "google-vertex" | "google-gemini-cli" => {
            let mut detector = spec.clone();
            detector.as_object_mut()?.remove("compat");
            resolve_simple(&detector, axes, api)
        }
        _ => return None,
    };
    Some(record.values)
}

/// Apply reviewed upstream metadata corrections in the fixed upstream order:
/// long-context tier derives from the live row, then cost/limit patches win.
pub fn apply_catalog_corrections(model: &mut Value, catalog: &Value) {
    if let Some(long) = catalog.get("longContext").filter(|v| v.is_object()) {
        let threshold = number(long, "inputThreshold");
        let multiplier = number(long, "multiplier");
        let base = model.get("cost").cloned().unwrap_or_else(object);
        let has_price = ["input", "output", "cacheRead", "cacheWrite"].iter().any(|k| number(&base, k) != Some(0.0));
        if let (Some(threshold), Some(multiplier)) = (threshold, multiplier) {
            if has_price {
                let mut rates = json!({"inputThreshold":threshold});
                if long["inputThresholdInclusive"] == true {
                    rates["inputThresholdInclusive"] = json!(true);
                }
                for key in ["input", "output", "cacheRead", "cacheWrite"] {
                    rates[key] = num(number(&base, key).unwrap_or(f64::NAN) * multiplier);
                }
                if !model["cost"].is_object() {
                    model["cost"] = object();
                }
                model["cost"]["longContext"] = rates;
            } else if ["input", "output", "cacheRead", "cacheWrite"].iter().all(|k| number(long, k).is_some()) {
                let mut rates = json!({"inputThreshold":threshold});
                for key in ["input", "output", "cacheRead", "cacheWrite"] {
                    rates[key] = long[key].clone();
                }
                if !model["cost"].is_object() {
                    model["cost"] = object();
                }
                model["cost"]["longContext"] = rates;
            }
        } else if let Some(threshold) = threshold
            && ["input", "output", "cacheRead", "cacheWrite"].iter().all(|k| number(long, k).is_some())
        {
            let mut rates = json!({"inputThreshold":threshold});
            for key in ["input", "output", "cacheRead", "cacheWrite"] {
                rates[key] = long[key].clone();
            }
            if !model["cost"].is_object() {
                model["cost"] = object();
            }
            model["cost"]["longContext"] = rates;
        }
    }
    if let Some(patch) = catalog.get("costPatch").filter(|v| v.is_object()) {
        if !model["cost"].is_object() {
            model["cost"] = object();
        }
        for key in ["input", "output", "cacheRead", "cacheWrite"] {
            if let Some(v) = patch.get(key).filter(|v| v.is_number()) {
                model["cost"][key] = v.clone();
            }
        }
    }
    if let Some(patch) = catalog.get("limitsPatch").filter(|v| v.is_object()) {
        for key in ["contextWindow", "maxTokens"] {
            if let Some(v) = patch.get(key).filter(|v| v.is_number()) {
                model[key] = v.clone();
            }
        }
    }
    if let Some(floor) = number(catalog, "contextWindowFloor") {
        model["contextWindow"] = num(number(model, "contextWindow").unwrap_or(0.0).max(floor));
    }
    if let Some(input) = catalog
        .get("inputModalities")
        .and_then(Value::as_array)
        .filter(|v| v.iter().all(|x| x == "text" || x == "image"))
    {
        model["input"] = json!(input);
    }
}
fn apply_catalog_assignments(model: &mut Value, catalog: &Value) {
    if let Some(tier) = catalog.get("serviceTierCost").filter(|v| v.is_object()) {
        let mut out = object();
        for key in ["flex", "priority"] {
            if let Some(v) = tier.get(key).filter(|v| v.is_number()) {
                out[key] = v.clone();
            }
        }
        model["serviceTierCost"] = out;
    }
    if let Some(v) = catalog.get("priority").filter(|v| v.is_number()) {
        model["priority"] = v.clone();
    }
    if let Some(v) = catalog.get("applyPatchToolType").filter(|v| *v == "freeform" || *v == "function") {
        model["applyPatchToolType"] = v.clone();
    }
    if catalog["requiresCursorToolSchemaProjection"] == true {
        model["requiresCursorToolSchemaProjection"] = json!(true);
    } else {
        model.as_object_mut().expect("model").remove("requiresCursorToolSchemaProjection");
    }
    if !defined(model, "contextPromotionTarget")
        && let Some(v) = catalog.get("contextPromotionTarget").filter(|v| v.is_string())
    {
        model["contextPromotionTarget"] = v.clone();
    }
}
pub(crate) fn direct_openai_responses_endpoint(spec: &Value) -> bool {
    let api = string(spec, "api");
    let provider = string(spec, "provider");
    let base_url = string(spec, "baseUrl");
    if api == "openai-responses" {
        if provider != "openai" {
            return false;
        }
        return base_url.is_empty()
            || reqwest::Url::parse(base_url)
                .is_ok_and(|u| u.scheme() == "https" && u.host_str() == Some("api.openai.com"));
    }
    if api != "azure-openai-responses" || !matches!(provider, "azure" | "azure-openai") {
        return false;
    }
    base_url.is_empty()
        || reqwest::Url::parse(base_url).is_ok_and(|u| {
            u.scheme() == "https"
                && u.host_str()
                    .is_some_and(|h| h.ends_with(".openai.azure.com") || h == "models.inference.ai.azure.com")
        })
}
fn supports_openai_ga_computer_use(
    spec: &Value,
    identity: &Value,
    explicit: Option<&Value>,
) -> Result<Value, CatalogPolicyError> {
    if let Some(v) = explicit {
        return Ok(v.clone());
    }
    if !direct_openai_responses_endpoint(spec) {
        return Ok(json!(false));
    }
    let wire_identity = if let Some(id) = spec.get("requestModelId") {
        let mut wire_spec = spec.clone();
        wire_spec["id"] = id.clone();
        resolve_model_policy(&wire_spec)?["identity"].clone()
    } else {
        identity.clone()
    };
    let revision =
        string(&wire_identity, "revision").split('.').map(|v| v.parse::<f64>().unwrap_or(f64::NAN)).collect::<Vec<_>>();
    let major = revision.first().copied().unwrap_or(0.0);
    let minor = revision.get(1).copied().unwrap_or(0.0);
    Ok(json!(string(&wire_identity, "class") == "openai" && (major > 5.0 || (major == 5.0 && minor >= 4.0))))
}

/// Construct a model without dropping authored metadata or unknown fields.
pub fn build_model(spec: &Value) -> Result<Value, CatalogPolicyError> {
    let policy = resolve_model_policy(spec)?;
    let name = spec
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| CatalogPolicyError("Model name must be a string".into()))?;
    let explicit = if defined(spec, "supportsComputerUseConfig") {
        spec.get("supportsComputerUseConfig").filter(|v| v.is_boolean())
    } else {
        spec.get("supportsComputerUse")
    };
    let mut model = spec.clone();
    model["name"] = json!(clean_model_name(name));
    model["identity"] = policy["identity"].clone();
    model["requiresGlyphTokenization"] = json!(string(&policy["identity"], "class") == "anthropic");
    let tokenizer = if let Some(tokenizer) = spec.get("tokenizer").filter(|v| !v.is_null()) {
        Some(tokenizer.clone())
    } else {
        let id = spec
            .get("requestModelId")
            .filter(|v| !v.is_null())
            .and_then(Value::as_str)
            .unwrap_or_else(|| string(spec, "id"));
        resolve_model_tokenizer(id)?.map(|v| json!(v))
    };
    for key in ["thinking", "compat"] {
        if let Some(v) = policy.get(key) {
            model[key] = v.clone();
        } else {
            model.as_object_mut().expect("model object").remove(key);
        }
    }
    model["supportsComputerUse"] = supports_openai_ga_computer_use(spec, &policy["identity"], explicit)?;
    if let Some(v) = explicit {
        model["supportsComputerUseConfig"] = v.clone();
    } else {
        model.as_object_mut().expect("model").remove("supportsComputerUseConfig");
    }
    if let Some(v) = tokenizer {
        model["tokenizer"] = v;
    } else {
        model.as_object_mut().expect("model").remove("tokenizer");
    }
    if let Some(v) = spec.get("compat") {
        model["compatConfig"] = v.clone();
    } else {
        model.as_object_mut().expect("model").remove("compatConfig");
    }
    apply_catalog_assignments(&mut model, &policy["catalog"]);
    apply_catalog_corrections(&mut model, &policy["catalog"]);
    Ok(model)
}

fn context_target_thinking_level(suffix: &str) -> Option<&'static str> {
    // Fixed coding-agent/thinking.ts accepts exact selectors or an
    // unambiguous prefix of at least two characters, without case folding.
    const LEVELS: &[&str] = &["inherit", "off", "minimal", "low", "medium", "high", "xhigh", "max"];
    if let Some(level) = LEVELS.iter().copied().find(|level| *level == suffix) {
        return Some(level);
    }
    if suffix.len() < 2 {
        return None;
    }
    let mut matches = LEVELS.iter().copied().filter(|level| level.starts_with(suffix));
    let level = matches.next()?;
    matches.next().is_none().then_some(level)
}

fn configured_context_target<'a>(current: &Value, candidates: &'a [Value]) -> Option<&'a Value> {
    let target = js_trim(current.get("contextPromotionTarget")?.as_str()?);
    if target.is_empty() {
        return None;
    }
    let find = |provider: &str, id: &str| {
        candidates.iter().find(|model| {
            model.get("provider").and_then(Value::as_str) == Some(provider)
                && model.get("id").and_then(Value::as_str) == Some(id)
        })
    };
    if let Some((provider, id)) = target.split_once('/')
        && !provider.is_empty()
    {
        let parsed_id = match id.rsplit_once(':') {
            Some((base, suffix)) => match context_target_thinking_level(suffix) {
                Some("max") => {
                    if find(provider, id).is_some() {
                        id
                    } else {
                        base
                    }
                }
                Some(_) => base,
                None if suffix == "auto" => {
                    if find(provider, id).is_some() {
                        id
                    } else {
                        base
                    }
                }
                None => id,
            },
            None => id,
        };
        if let Some(explicit) = find(provider, parsed_id) {
            return Some(explicit);
        }
    }
    find(current.get("provider")?.as_str()?, target)
}

/// Resolve fixed OMP context-promotion metadata without preparing a route.
///
/// Port of session/role-models.ts::resolveContextPromotionConfiguredTarget
/// and session-maintenance.ts::resolveContextPromotionTarget at the pinned
/// SHA in this module header. Exact qualified selection precedes a literal
/// same-provider fallback. A target must have a strictly larger known window.
///
/// `candidates` is caller-supplied metadata, not an authenticated/executable
/// registry. The Host owns enabled settings, stale-turn checks, target route
/// preparation, usable credentials and adoption on the existing Session.
pub fn resolve_context_promotion_target<'a>(current: &Value, candidates: &'a [Value]) -> Option<&'a Value> {
    let window = current.get("contextWindow")?.as_f64().filter(|window| window.is_finite() && *window > 0.0)?;
    let candidate = configured_context_target(current, candidates)?;
    if candidate.get("provider")?.as_str()? == current.get("provider")?.as_str()?
        && candidate.get("id")?.as_str()? == current.get("id")?.as_str()?
    {
        return None;
    }
    candidate.get("contextWindow")?.as_f64().filter(|target| target.is_finite() && *target > window)?;
    Some(candidate)
}

/// Exact embedded-tokenizer selection (pure; caching is not observable).
pub fn resolve_model_tokenizer(model_id: &str) -> Result<Option<&'static str>, CatalogPolicyError> {
    let bare = model_id.rsplit('/').next().unwrap_or(model_id);
    let identity = classify_model("", bare, ClassifyOptions { observed_at_ms: None, lenient: true })
        .map_err(|e| CatalogPolicyError(e.to_string()))?;
    let facts = IdentityFacts::new(&identity);
    let tokenizer = if facts.is("anthropic") {
        Some(if facts.family(&["opus"]) {
            if facts.rev_gte("5") {
                "claude-v5"
            } else if facts.rev_gte("4.7") {
                "claude-v47"
            } else {
                "claude-v3"
            }
        } else if facts.family(&["sonnet", "fable", "mythos"]) && facts.rev_gte("5") {
            "claude-v5-sonnet"
        } else {
            "claude-v3"
        })
    } else if facts.is("qwen") && facts.rev_gte("3.5") {
        Some("qwen3")
    } else if facts.is("deepseek") {
        Some("deepseek-v3")
    } else if facts.is("kimi") {
        Some("kimi-k2")
    } else if facts.is("glm") && facts.rev_gte("5") {
        Some("glm5")
    } else {
        None
    };
    Ok(tokenizer)
}

pub fn clean_model_name(name: &str) -> String {
    static AUTHOR: OnceLock<Regex> = OnceLock::new();
    static NOISE: OnceLock<Regex> = OnceLock::new();
    static SPACES: OnceLock<Regex> = OnceLock::new();
    let author = AUTHOR.get_or_init(|| regex(r"^[A-Za-z][A-Za-z0-9 .+&'-]{0,23}: "));
    let noise = NOISE.get_or_init(|| regex(r"[\x09-\x0D\x20\u00A0\u1680\u2000-\u200A\u2028\u2029\u202F\u205F\u3000\uFEFF]*\((?:latest|Antigravity|\$+|>?[0-9]+% off|retires [^)]*)\)"));
    let spaces = SPACES.get_or_init(|| regex(r" {2,}"));
    let cleaned = author.replace(name, "");
    let cleaned = noise.replace_all(&cleaned, "");
    let cleaned = spaces.replace_all(&cleaned, " ");
    let cleaned = js_trim(&cleaned);
    if cleaned.is_empty() { name.to_owned() } else { cleaned.to_owned() }
}
fn radix_number(s: &str, radix: u32) -> Option<f64> {
    if s.is_empty() {
        return None;
    }
    let width = radix.trailing_zeros() as usize;
    let mut bits = Vec::with_capacity(s.len().saturating_mul(width));
    for ch in s.chars() {
        if !ch.is_ascii() {
            return None;
        }
        let digit = ch.to_digit(radix)?;
        for bit in (0..width).rev() {
            bits.push((digit >> bit) & 1);
        }
    }
    let Some(first) = bits.iter().position(|v| *v != 0) else {
        return Some(0.0);
    };
    let bits = &bits[first..];
    let take = bits.len().min(53);
    let mut mantissa = bits[..take].iter().fold(0u64, |v, bit| (v << 1) | u64::from(*bit));
    if bits.len() > 53 && bits[53] == 1 && (bits[54..].contains(&1) || mantissa & 1 != 0) {
        mantissa += 1;
    }
    let exponent = bits.len().saturating_sub(take);
    if exponent > 1024 {
        return None;
    }
    let value = mantissa as f64 * 2f64.powi(exponent as i32);
    value.is_finite().then_some(value)
}
pub fn to_number(value: &Value) -> Option<f64> {
    let parsed = if let Some(n) = value.as_f64() {
        n
    } else {
        let s = js_trim(value.as_str()?);
        if s.is_empty() {
            return None;
        }
        if let Some(v) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
            radix_number(v, 16)?
        } else if let Some(v) = s.strip_prefix("0b").or_else(|| s.strip_prefix("0B")) {
            radix_number(v, 2)?
        } else if let Some(v) = s.strip_prefix("0o").or_else(|| s.strip_prefix("0O")) {
            radix_number(v, 8)?
        } else {
            s.parse::<f64>().ok()?
        }
    };
    parsed.is_finite().then_some(parsed)
}
pub fn to_positive_number(value: &Value, fallback: Option<f64>) -> Option<f64> {
    to_number(value).filter(|v| *v > 0.0).or(fallback)
}
pub fn to_positive_number_or_null(value: &Value) -> Option<f64> {
    to_positive_number(value, None)
}
pub fn to_boolean(value: &Value) -> Option<bool> {
    value.as_bool()
}
/// The catalog utils re-export of the fixed utils `isRecord` type guard.
pub fn is_record(value: &Value) -> bool {
    value.is_object()
}
pub fn is_anthropic_oauth_token(key: &str) -> bool {
    key.contains("sk-ant-oat")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(api: &str, provider: &str, id: &str) -> Value {
        json!({"api":api,"provider":provider,"id":id,"name":"OpenAI: A model (latest)",
            "reasoning":true,"input":["text"],"contextWindow":4096,"maxTokens":1024,
            "cost":{"input":1,"output":2,"cacheRead":0.1,"cacheWrite":0.2},"privateMetadata":{"original":true}})
    }

    #[test]
    fn native_context_promotion_selectors_and_window_guards() {
        // Fixed role-models.ts exact selection and model-resolver.ts thinking
        // selectors; context-promotion.test.ts also pins same-window no-op.
        let candidates = vec![
            json!({"provider":"p","id":"large","contextWindow":200}),
            json!({"provider":"q","id":"large","contextWindow":300}),
            json!({"provider":"p","id":"org/large","contextWindow":400}),
            json!({"provider":"org","id":"large","contextWindow":500}),
            json!({"provider":"p","id":"large:max","contextWindow":250}),
            json!({"provider":"p","id":"large:auto","contextWindow":260}),
            json!({"provider":"p","id":"large:ma","contextWindow":270}),
            json!({"provider":"p","id":"large:high","contextWindow":280}),
            json!({"provider":"p","id":"missing/large","contextWindow":290}),
            json!({"provider":"p","id":"small","contextWindow":1000}),
        ];
        for (target, expected) in [
            ("p/large", Some(0)),
            ("large", Some(0)),
            ("\u{feff} p/large \u{2029}", Some(0)),
            ("org/large", Some(3)),
            ("p/org/large", Some(2)),
            ("missing/large", Some(8)),
            ("p/large:hi", Some(0)),
            ("p/large:high", Some(0)),
            ("p/large:max", Some(4)),
            ("p/large:ma", Some(6)),
            ("p/large:auto", Some(5)),
            ("q/large:max", Some(1)),
            ("q/large:auto", Some(1)),
            ("q/large:xhi", Some(1)),
            ("q/large:in", Some(1)),
            ("q/large:mi", Some(1)),
            ("q/large:of", Some(1)),
            ("large:high", Some(7)),
            ("q/large:m", None),
            ("large:hi", None),
            ("p/*", None),
            ("p/LARGE", None),
            ("p/ large", None),
            ("small", None),
            ("", None),
            ("missing", None),
        ] {
            let current = json!({"provider":"p","id":"small","contextWindow":100,"contextPromotionTarget":target});
            let selected = resolve_context_promotion_target(&current, &candidates);
            assert_eq!(selected, expected.map(|index| &candidates[index]), "{target}");
            if let (Some(selected), Some(index)) = (selected, expected) {
                assert!(std::ptr::eq(selected, &candidates[index]), "metadata stays caller-owned");
            }
        }
        for source_window in [Value::Null, json!(0), json!(-1), json!(200), json!("100")] {
            let current =
                json!({"provider":"p","id":"small","contextWindow":source_window,"contextPromotionTarget":"large"});
            assert!(resolve_context_promotion_target(&current, &candidates).is_none());
        }
        let current = json!({"provider":"p","id":"small","contextWindow":100,"contextPromotionTarget":"large"});
        assert!(resolve_context_promotion_target(&current, &[]).is_none());
        for target_window in [Value::Null, json!(0), json!(-1), json!(100), json!("200")] {
            let candidate = json!({"provider":"p","id":"large","contextWindow":target_window});
            assert!(resolve_context_promotion_target(&current, &[candidate]).is_none());
        }
        let first = json!({"provider":"p","id":"large","contextWindow":100});
        assert!(resolve_context_promotion_target(&current, &[first, candidates[0].clone()]).is_none());
        let authored = json!({"api":"openai-responses","provider":"p","id":"small","name":"Small",
            "contextWindow":100,"contextPromotionTarget":"large"});
        assert_eq!(build_model(&authored).unwrap()["contextPromotionTarget"], "large");
    }

    #[test]
    fn authored_unknown_metadata_and_optional_compat_keys_survive_materialization() {
        let mut input = spec("openai-completions", "custom", "runtime-thinking-model");
        input["compat"] = json!({"extraBody":null,"madeUpCompat":true,"supportsDeveloperRole":false});
        input["thinking"] =
            json!({"mode":"budget","efforts":["low"],"supportsDisplay":false,"customThinking":{"retained":true}});
        let built = build_model(&input).unwrap();
        assert_eq!(built["privateMetadata"], input["privateMetadata"]);
        assert_eq!(built["compatConfig"], input["compat"]);
        assert_eq!(built["thinking"]["customThinking"], input["thinking"]["customThinking"]);
        assert_eq!(built["thinking"]["supportsDisplay"], false);
        assert_eq!(built["thinking"]["requiresEffort"], true);
        assert_eq!(built["compat"]["extraBody"], Value::Null);
        assert!(built["compat"].get("extraBody").is_some());
        assert!(built["compat"].get("madeUpCompat").is_none());
        assert_eq!(built["name"], "A model");
    }

    #[test]
    fn computer_use_follows_the_actual_wire_model_and_direct_tls_endpoint() {
        let mut input = spec("openai-responses", "openai", "gpt-5.4");
        assert_eq!(build_model(&input).unwrap()["supportsComputerUse"], true);
        input["requestModelId"] = json!("gpt-5.3");
        assert_eq!(build_model(&input).unwrap()["supportsComputerUse"], false);
        input.as_object_mut().unwrap().remove("requestModelId");
        input["baseUrl"] = json!("http://api.openai.com/v1");
        assert_eq!(build_model(&input).unwrap()["supportsComputerUse"], false);
        input["supportsComputerUseConfig"] = json!(true);
        assert_eq!(build_model(&input).unwrap()["supportsComputerUse"], true);
    }

    #[test]
    fn reviewed_cost_patch_follows_long_context_derivation() {
        let mut model = spec("openai-completions", "custom", "a");
        apply_catalog_corrections(
            &mut model,
            &json!({"longContext":{"inputThreshold":200000,"inputThresholdInclusive":true,"multiplier":2},
            "costPatch":{"input":3},"limitsPatch":{"contextWindow":2000,"maxTokens":3000},"contextWindowFloor":4000,"inputModalities":["text","image"]}),
        );
        assert_eq!(model["cost"]["input"], 3);
        assert_eq!(model["cost"]["longContext"]["input"].as_f64(), Some(2.0));
        assert_eq!(model["cost"]["longContext"]["inputThresholdInclusive"], true);
        assert_eq!(model["contextWindow"].as_f64(), Some(4000.0));
        assert_eq!(model["maxTokens"], 3000);
        assert_eq!(model["input"], json!(["text", "image"]));
    }

    #[test]
    fn endpoint_signature_checks_preserve_the_auth_origin_boundary() {
        assert!(is_official_anthropic_api_url(None));
        assert!(is_official_anthropic_api_url(Some("HTTPS://API.ANTHROPIC.COM/v1")));
        assert!(!is_official_anthropic_api_url(Some("https://api.anthropic.com.evil.com")));
        assert!(!is_official_anthropic_api_url(Some("https://api.anthropic.com:443")));
        assert!(is_anthropic_signing_proxy_url(Some("https://gateway.ai.cloudflare.com/v1/a/b/anthropic")));
        assert!(is_anthropic_signing_proxy_url(Some("https://my.services.ai.azure.com/models")));
        assert!(!is_anthropic_signing_proxy_url(Some("https://arbitrary-proxy.example")));
        for endpoint in [
            "https://ſ.services.ai.azure.com",
            "https://a.ſervices.ai.azure.com",
            "https://bedrocK-runtime.us-east-1.amazonaws.com",
        ] {
            assert!(!is_anthropic_signing_proxy_url(Some(endpoint)), "{endpoint}");
        }
    }

    #[test]
    fn sparse_overrides_preserve_inherited_key_and_null_prototype_order() {
        // Original-source JSON.parse captures: Object.prototype keys pass
        // `key in compat`; clearing its prototype changes later membership.
        let mut compat = object();
        let overrides: Value = serde_json::from_str(
            r#"{"toString":"x","constructor":1,"hasOwnProperty":false,"__proto__":null,"extraJunk":true}"#,
        )
        .unwrap();
        apply_compat_overrides(&mut compat, Some(&overrides));
        assert_eq!(compat, json!({"toString":"x","constructor":1,"hasOwnProperty":false}));
        let mut compat = object();
        let overrides: Value = serde_json::from_str(r#"{"__proto__":null,"toString":"x","constructor":1}"#).unwrap();
        apply_compat_overrides(&mut compat, Some(&overrides));
        assert_eq!(compat, object());
        let mut input = spec("openai-completions", "custom", "runtime-thinking-model");
        input["compat"] = serde_json::from_str(
            r#"{"__proto__":{"trustExplicitThinkingOnly":true,"authoredExtension":true},"authoredExtension":42}"#,
        )
        .unwrap();
        let policy = resolve_model_policy(&input).unwrap();
        assert_eq!(policy["compat"]["authoredExtension"], 42);
        assert!(policy["compat"].get("__proto__").is_none());
        assert!(policy.get("thinking").is_none());
    }

    #[test]
    fn display_name_and_numeric_conversions_use_ecmascript_whitespace_and_digits() {
        assert_eq!(clean_model_name("\u{feff}Model (20% off)\u{feff}"), "Model");
        assert_eq!(clean_model_name("Model (２０% off)"), "Model (２０% off)");
        assert_eq!(clean_model_name("Model\u{85}(latest)"), "Model\u{85}");
        assert_eq!(to_number(&json!("\u{feff}0x10000000000000000\u{feff}")), Some(18_446_744_073_709_551_616.0));
        assert_eq!(to_number(&json!("0x20000000000003")), Some(9_007_199_254_740_996.0));
        assert_eq!(to_number(&json!("")), None);
        assert_eq!(to_number(&json!("\u{85}1")), None);
        assert_eq!(to_number(&json!("Infinity")), None);
        assert_eq!(to_positive_number(&json!(-1), Some(9.0)), Some(9.0));
    }
}
