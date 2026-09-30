//! Pure models configuration validation from fixed OMP
//! 596f2da7101178214aa27a753529d15e6b7ad91d:
//! `packages/coding-agent/src/config/models-config-schema-bundle.ts`,
//! `packages/coding-agent/src/config/models-config.ts`, and finite-number,
//! object/array, extra-key and morph semantics in `packages/omptype/src/{interp,compile}.ts`.
//!
//! Unknown properties survive validation, except inside thinking's explicit
//! canonicalizing pipe. There is no config I/O, authentication, discovery or
//! runtime routing here. Errors retain the schema/provider stage and all
//! collected schema issues; Rust renders structured paths rather than OmpErrors.
//! JSON cannot represent undefined, NaN, Infinity, symbols or prototype objects.
//! Loaders must reject nonfinite YAML numbers before converting them to Value.
//!
//! MIT License
//! Copyright (c) 2025 Mario Zechner
//! Copyright (c) 2025-2026 Can Bölük
//! Copyright (c) 2026 Stencil Labs, Inc.
//! Permission is hereby granted, free of charge, to any person obtaining a copy
//! of this software and associated documentation files (the "Software"), to deal
//! in the Software without restriction, including without limitation the rights
//! to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//! copies of the Software, and to permit persons to whom the Software is
//! furnished to do so, subject to the following conditions:
//! The above copyright notice and this permission notice shall be included in
//! all copies or substantial portions of the Software.
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//! IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//! FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//! LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//! OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
//! THE SOFTWARE.

use serde_json::{Map, Value};
use std::fmt;

const APIS: &[&str] = &[
    "openai-completions",
    "openai-responses",
    "openai-codex-responses",
    "azure-openai-responses",
    "anthropic-messages",
    "bedrock-converse-stream",
    "google-generative-ai",
    "google-gemini-cli",
    "google-vertex",
];
const EFFORTS: &[&str] = &["minimal", "low", "medium", "high", "xhigh", "max"];
const THINKING_MODES: &[&str] = &["effort", "budget", "google-level", "anthropic-adaptive", "anthropic-budget-effort"];
const TOKENIZERS: &[&str] =
    &["claude-v3", "claude-v47", "claude-v5", "claude-v5-sonnet", "qwen3", "deepseek-v3", "kimi-k2", "glm5"];
const DISCOVERY_TYPES: &[&str] = &["ollama", "llama.cpp", "lm-studio", "openai-models-list", "proxy", "litellm"];
const OPENAI_COMPAT_BOOLEANS: &[&str] = &[
    "supportsStore",
    "supportsDeveloperRole",
    "supportsMultipleSystemMessages",
    "supportsReasoningEffort",
    "supportsUsageInStreaming",
    "requiresToolResultName",
    "requiresMistralToolIds",
    "requiresAssistantAfterToolResult",
    "requiresThinkingAsText",
    "requiresReasoningContentForToolCalls",
    "allowsSyntheticReasoningContentForToolCalls",
    "requiresAssistantContentForToolCalls",
    "supportsToolChoice",
    "supportsForcedToolChoice",
    "disableReasoningOnForcedToolChoice",
    "disableReasoningOnToolChoice",
    "qwenTemplateReasoningEffort",
    "supportsStrictMode",
    "supportsLongPromptCacheRetention",
    "supportsReasoningParams",
    "supportsReasoningSummary",
    "alwaysSendMaxTokens",
    "strictResponsesPairing",
    "supportsImageDetailOriginal",
    "supportsContextManagement",
    "supportsEagerToolInputStreaming",
    "allowAnthropicHeaderOverrides",
    "requiresToolResultId",
    "replayUnsignedThinking",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigValidationStage {
    Schema,
    Provider,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigValidationIssue {
    /// JSON pointer (escaped property names); empty for a provider business error.
    pub path: String,
    pub expected: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigValidationError {
    pub stage: ConfigValidationStage,
    pub issues: Vec<ConfigValidationIssue>,
}

impl fmt::Display for ConfigValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, issue) in self.issues.iter().enumerate() {
            if index != 0 {
                f.write_str("; ")?;
            }
            if issue.path.is_empty() {
                f.write_str(&issue.expected)?;
            } else {
                write!(f, "{}: must be {}", issue.path, issue.expected)?;
            }
        }
        Ok(())
    }
}
impl std::error::Error for ConfigValidationError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValidationMode {
    ModelsConfig,
    RuntimeRegister,
}

/// Owned, normalized configuration with its unknown JSON properties intact.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelsConfig(Value);

impl ModelsConfig {
    /// Schema plus the fixed file-mode business validator, like ModelsConfigFile.
    pub fn validate(value: Value) -> Result<Self, ConfigValidationError> {
        let config = Self::validate_schema(value)?;
        if let Some(providers) = config.0.get("providers").and_then(Value::as_object) {
            for (provider, value) in ara_prompt::js::entries(providers) {
                // The source file wrapper supplies models ?? [] to the required
                // business-validator interface, without inserting it into output.
                let mut projected = value.as_object().expect("schema normalized provider").clone();
                projected.entry("models").or_insert_with(|| Value::Array(Vec::new()));
                validate_provider_configuration(provider, &Value::Object(projected), ValidationMode::ModelsConfig)?;
            }
        }
        Ok(config)
    }

    /// Only the omptype schema and thinking morph, before business validation.
    pub fn validate_schema(mut value: Value) -> Result<Self, ConfigValidationError> {
        let mut validator = Validator::default();
        if validator.morph_object(&mut value, "")
            && let Some(providers) = value.get_mut("providers")
            && validator.morph_object(providers, "/providers")
        {
            for (provider, config) in providers.as_object_mut().expect("morphed").iter_mut() {
                validator.provider(config, &child("/providers", provider));
            }
        }
        if validator.issues.is_empty() {
            Ok(Self(value))
        } else {
            Err(ConfigValidationError { stage: ConfigValidationStage::Schema, issues: validator.issues })
        }
    }

    pub fn value(&self) -> &Value {
        &self.0
    }
}

fn child(path: &str, key: &str) -> String {
    format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"))
}

#[derive(Clone, Copy)]
enum Kind {
    String,
    Boolean,
    Number,
    NonNegative,
    Enum(&'static [&'static str]),
    StringArray,
    EnumArray(&'static [&'static str]),
}

#[derive(Default)]
struct Validator {
    issues: Vec<ConfigValidationIssue>,
}

impl Validator {
    fn issue(&mut self, path: &str, expected: impl Into<String>) {
        self.issues.push(ConfigValidationIssue { path: path.to_owned(), expected: expected.into() });
    }

    // omptype object accepts arrays, while array schemas still require arrays.
    fn object(&mut self, value: &Value, path: &str) -> bool {
        if matches!(value, Value::Object(_) | Value::Array(_)) {
            true
        } else {
            self.issue(path, "an object");
            false
        }
    }

    // A subtree with thinking morphs spreads its object-like input into a fresh
    // object even when no thinking property is present (omptype hasMorph).
    fn morph_object(&mut self, value: &mut Value, path: &str) -> bool {
        if !self.object(value, path) {
            return false;
        }
        if let Value::Array(values) = value {
            *value = Value::Object(values.iter().enumerate().map(|(i, v)| (i.to_string(), v.clone())).collect());
        }
        true
    }

    fn check(&mut self, value: &Value, path: &str, kind: Kind) {
        let valid = match kind {
            Kind::String => value.is_string(),
            Kind::Boolean => value.is_boolean(),
            Kind::Number => value.as_f64().is_some_and(f64::is_finite),
            Kind::NonNegative => value.as_f64().is_some_and(|n| n.is_finite() && n >= 0.0),
            Kind::Enum(values) => value.as_str().is_some_and(|value| values.contains(&value)),
            Kind::StringArray | Kind::EnumArray(_) => {
                let Some(values) = value.as_array() else {
                    self.issue(path, "an array");
                    return;
                };
                for (index, value) in values.iter().enumerate() {
                    let element = match kind {
                        Kind::EnumArray(allowed) => Kind::Enum(allowed),
                        _ => Kind::String,
                    };
                    self.check(value, &child(path, &index.to_string()), element);
                }
                return;
            }
        };
        if !valid {
            self.issue(
                path,
                match kind {
                    Kind::String => "a string".into(),
                    Kind::Boolean => "a boolean".into(),
                    Kind::Number => "a finite number".into(),
                    Kind::NonNegative => "a finite number >= 0".into(),
                    Kind::Enum(values) => {
                        values.iter().map(|value| format!("\"{value}\"")).collect::<Vec<_>>().join(" | ")
                    }
                    _ => unreachable!(),
                },
            );
        }
    }

    fn optional(&mut self, object: &Value, path: &str, name: &str, kind: Kind) {
        if let Some(value) = object.get(name) {
            self.check(value, &child(path, name), kind);
        }
    }

    fn required(&mut self, object: &Value, path: &str, name: &str, kind: Kind) {
        if let Some(value) = object.get(name) {
            self.check(value, &child(path, name), kind);
        } else {
            self.issue(&child(path, name), "a required property");
        }
    }

    fn nonempty(&mut self, object: &Value, path: &str, fields: &[&str]) {
        // Narrowing runs after the structural schema; whitespace is not trimmed.
        for name in fields {
            if object.get(*name).and_then(Value::as_str) == Some("") {
                self.issue(path, format!("{name} a non-empty string"));
                return;
            }
        }
    }

    fn record(&mut self, value: &Value, path: &str, kind: Option<Kind>) {
        if !self.object(value, path) {
            return;
        }
        if let Some(kind) = kind {
            match value {
                Value::Object(values) => {
                    for (key, value) in ara_prompt::js::entries(values) {
                        self.check(value, &child(path, key), kind);
                    }
                }
                Value::Array(values) => {
                    for (index, value) in values.iter().enumerate() {
                        self.check(value, &child(path, &index.to_string()), kind);
                    }
                }
                _ => unreachable!(),
            }
        }
    }

    fn effort_map(&mut self, value: &Value, path: &str) {
        if self.object(value, path) {
            for effort in EFFORTS {
                self.optional(value, path, effort, Kind::String);
            }
        }
    }

    fn compat(&mut self, value: &Value, path: &str, with_bedrock: bool) {
        if !self.object(value, path) {
            return;
        }
        for field in OPENAI_COMPAT_BOOLEANS {
            self.optional(value, path, field, Kind::Boolean);
        }
        for (field, choices) in [
            ("maxTokensField", &["max_completion_tokens", "max_tokens"][..]),
            ("reasoningContentField", &["reasoning_content", "reasoning", "reasoning_text"][..]),
            ("thinkingFormat", &["openai", "openrouter", "zai", "qwen", "qwen-chat-template"][..]),
            ("cacheControlFormat", &["anthropic"][..]),
            ("toolStrictMode", &["all_strict", "none"][..]),
            ("streamMarkupHealingPattern", &["kimi", "dsml", "qwen", "thinking"][..]),
        ] {
            self.optional(value, path, field, Kind::Enum(choices));
        }
        self.optional(value, path, "streamIdleTimeoutMs", Kind::NonNegative);
        if let Some(map) = value.get("reasoningEffortMap") {
            self.effort_map(map, &child(path, "reasoningEffortMap"));
        }
        for field in ["openRouterRouting", "vercelGatewayRouting"] {
            if let Some(routing) = value.get(field) {
                let path = child(path, field);
                if self.object(routing, &path) {
                    for field in ["only", "order"] {
                        self.optional(routing, &path, field, Kind::StringArray);
                    }
                }
            }
        }
        if let Some(body) = value.get("extraBody") {
            self.record(body, &child(path, "extraBody"), None);
        }
        if with_bedrock {
            // whenThinking is OpenAICompatFieldsSchema, not recursively another
            // OpenAICompatSchema or a Bedrock schema. Its unknown keys survive.
            if let Some(conditional) = value.get("whenThinking") {
                self.compat(conditional, &child(path, "whenThinking"), false);
            }
            self.optional(value, path, "promptCacheMode", Kind::Enum(&["none", "automatic", "explicit"]));
            // Shared by both intersected schemas, already checked above.
            for field in ["promptCacheMinimumTokens", "promptCacheMaximumCheckpoints"] {
                self.optional(value, path, field, Kind::NonNegative);
            }
        }
    }

    fn remote_compaction(&mut self, value: &Value, path: &str) {
        if !self.object(value, path) {
            return;
        }
        let before = self.issues.len();
        for field in ["enabled", "v2StreamingEnabled"] {
            self.optional(value, path, field, Kind::Boolean);
        }
        self.optional(value, path, "api", Kind::Enum(APIS));
        let strings = ["endpoint", "model", "v2Endpoint", "streamingEndpoint"];
        for field in strings {
            self.optional(value, path, field, Kind::String);
        }
        if self.issues.len() == before {
            self.nonempty(value, path, &strings);
        }
    }

    fn discovery(&mut self, value: &Value, path: &str) {
        if !self.object(value, path) {
            return;
        }
        let before = self.issues.len();
        self.required(value, path, "type", Kind::Enum(DISCOVERY_TYPES));
        self.optional(value, path, "timeoutMs", Kind::Number);
        self.optional(value, path, "injectV1", Kind::Boolean);
        if self.issues.len() != before {
            return;
        }
        if value.get("injectV1").is_some() && value.get("type").and_then(Value::as_str) != Some("openai-models-list") {
            self.issue(path, "injectV1 only on openai-models-list discovery");
            return;
        }
        if value.get("timeoutMs").and_then(Value::as_f64).is_some_and(|n| n <= 0.0) {
            self.issue(path, "timeoutMs a positive finite number");
        }
    }

    fn thinking(&mut self, value: &mut Value, path: &str) {
        if !self.object(value, path) {
            return;
        }
        let before = self.issues.len();
        self.required(value, path, "mode", Kind::Enum(THINKING_MODES));
        for field in ["efforts", "levels"] {
            self.optional(value, path, field, Kind::EnumArray(EFFORTS));
        }
        for field in ["defaultLevel", "minLevel", "maxLevel"] {
            self.optional(value, path, field, Kind::Enum(EFFORTS));
        }
        for field in ["supportsDisplay", "requiresEffort"] {
            self.optional(value, path, field, Kind::Boolean);
        }
        if let Some(map) = value.get("effortMap") {
            self.effort_map(map, &child(path, "effortMap"));
        }
        if self.issues.len() != before {
            return;
        }
        let efforts = if let Some(efforts) = value.get("efforts").or_else(|| value.get("levels")) {
            efforts.clone()
        } else if let (Some(min), Some(max)) =
            (value.get("minLevel").and_then(Value::as_str), value.get("maxLevel").and_then(Value::as_str))
        {
            let min = EFFORTS.iter().position(|level| *level == min).expect("checked effort");
            let max = EFFORTS.iter().position(|level| *level == max).expect("checked effort").max(min);
            Value::Array(EFFORTS[min..=max].iter().map(|level| Value::String((*level).into())).collect())
        } else {
            self.issue(path, "thinking with `efforts` (or legacy `levels`/`minLevel`+`maxLevel`)");
            return;
        };
        let mut normalized = Map::new();
        normalized.insert("mode".into(), value["mode"].clone());
        normalized.insert("efforts".into(), efforts);
        for field in ["defaultLevel", "effortMap", "supportsDisplay", "requiresEffort"] {
            if let Some(field_value) = value.get(field) {
                normalized.insert(field.into(), field_value.clone());
            }
        }
        *value = Value::Object(normalized);
    }

    fn model(&mut self, value: &mut Value, path: &str, is_override: bool) {
        if !self.morph_object(value, path) {
            return;
        }
        let before = self.issues.len();
        if !is_override {
            self.required(value, path, "id", Kind::String);
            self.optional(value, path, "api", Kind::Enum(APIS));
            self.optional(value, path, "baseUrl", Kind::String);
        }
        self.optional(value, path, "name", Kind::String);
        for field in ["reasoning", "supportsTools", "omitMaxOutputTokens", "preferWebsockets"] {
            self.optional(value, path, field, Kind::Boolean);
        }
        self.optional(value, path, "input", Kind::EnumArray(&["text", "image"]));
        self.optional(value, path, "imageInputDecoder", Kind::Enum(&["stb"]));
        self.optional(value, path, "tokenizer", Kind::Enum(TOKENIZERS));
        for field in ["premiumMultiplier", "contextWindow", "maxTokens"] {
            self.optional(value, path, field, Kind::Number);
        }
        for field in ["contextPromotionTarget", "compactionModel"] {
            self.optional(value, path, field, Kind::String);
        }
        if let Some(cost) = value.get("cost") {
            let path = child(path, "cost");
            if self.object(cost, &path) {
                for field in ["input", "output", "cacheRead", "cacheWrite"] {
                    if is_override {
                        self.optional(cost, &path, field, Kind::Number);
                    } else {
                        self.required(cost, &path, field, Kind::Number);
                    }
                }
            }
        }
        if let Some(headers) = value.get("headers") {
            self.record(headers, &child(path, "headers"), Some(Kind::String));
        }
        if let Some(compat) = value.get("compat") {
            self.compat(compat, &child(path, "compat"), true);
        }
        if let Some(remote) = value.get("remoteCompaction") {
            self.remote_compaction(remote, &child(path, "remoteCompaction"));
        }
        if let Some(thinking) = value.get_mut("thinking") {
            self.thinking(thinking, &child(path, "thinking"));
        }
        if self.issues.len() == before {
            let fields = if is_override {
                &["name", "contextPromotionTarget", "compactionModel"][..]
            } else {
                &["id", "name", "baseUrl", "contextPromotionTarget", "compactionModel"][..]
            };
            self.nonempty(value, path, fields);
        }
    }

    fn provider(&mut self, value: &mut Value, path: &str) {
        if !self.morph_object(value, path) {
            return;
        }
        let before = self.issues.len();
        for field in ["baseUrl", "apiKey", "guardrailIdentifier", "guardrailVersion"] {
            self.optional(value, path, field, Kind::String);
        }
        self.optional(value, path, "api", Kind::Enum(APIS));
        self.optional(value, path, "auth", Kind::Enum(&["apiKey", "none", "oauth"]));
        self.optional(value, path, "guardrailTrace", Kind::Enum(&["enabled", "disabled", "enabled_full"]));
        self.optional(value, path, "transport", Kind::Enum(&["pi-native"]));
        for field in ["authHeader", "disableStrictTools"] {
            self.optional(value, path, field, Kind::Boolean);
        }
        for field in ["headers", "requestMetadata"] {
            if let Some(record) = value.get(field) {
                self.record(record, &child(path, field), Some(Kind::String));
            }
        }
        if let Some(compat) = value.get("compat") {
            self.compat(compat, &child(path, "compat"), true);
        }
        if let Some(remote) = value.get("remoteCompaction") {
            self.remote_compaction(remote, &child(path, "remoteCompaction"));
        }
        if let Some(discovery) = value.get("discovery") {
            self.discovery(discovery, &child(path, "discovery"));
        }
        if let Some(models) = value.get_mut("models") {
            let path = child(path, "models");
            if let Some(models) = models.as_array_mut() {
                for (index, model) in models.iter_mut().enumerate() {
                    self.model(model, &child(&path, &index.to_string()), false);
                }
            } else {
                self.issue(&path, "an array");
            }
        }
        if let Some(overrides) = value.get_mut("modelOverrides") {
            let path = child(path, "modelOverrides");
            if self.morph_object(overrides, &path) {
                for (id, model) in overrides.as_object_mut().expect("morphed").iter_mut() {
                    self.model(model, &child(&path, id), true);
                }
            }
        }
        if self.issues.len() == before {
            self.nonempty(value, path, &["baseUrl", "apiKey"]);
        }
    }
}

fn provider_error(message: String) -> ConfigValidationError {
    ConfigValidationError {
        stage: ConfigValidationStage::Provider,
        issues: vec![ConfigValidationIssue { path: String::new(), expected: message }],
    }
}

fn truthy_field(value: &Value, field: &str) -> bool {
    value.get(field).is_some_and(ara_prompt::js::truthy)
}
fn property_string(value: Option<&Value>) -> String {
    match value {
        None => "undefined".into(),
        Some(Value::Null) => "null".into(),
        Some(value) => ara_prompt::js::to_string(value),
    }
}
fn numeric(value: &Value) -> f64 {
    // The shared template utility uses Null for undefined; actual JSON null
    // instead has JS ToNumber(null) == 0 in this business validator.
    if value.is_null() { 0.0 } else { ara_prompt::js::to_number(value) }
}

/// The fixed business validator over a typed-JSON projection. `models` is
/// required and must be an array, as in ProviderValidationConfig; file callers
/// default an omitted models property to [] before calling. This function does
/// not apply the file schema to runtime-register configurations.
pub fn validate_provider_configuration(
    provider: &str,
    config: &Value,
    mode: ValidationMode,
) -> Result<(), ConfigValidationError> {
    let models = config
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| provider_error(format!("Provider {provider}: \"models\" must be an array.")))?;
    let has_api = truthy_field(config, "api");
    if models.is_empty() {
        if mode == ValidationMode::ModelsConfig {
            let has_overrides = config.get("modelOverrides").is_some_and(|value| match value {
                Value::Object(values) => !values.is_empty(),
                Value::Array(values) => !values.is_empty(),
                Value::String(value) => !value.is_empty(),
                _ => false,
            });
            if ![
                "baseUrl",
                "headers",
                "compat",
                "apiKey",
                "disableStrictTools",
                "guardrailIdentifier",
                "requestMetadata",
                "remoteCompaction",
                "discovery",
            ]
            .iter()
            .any(|field| truthy_field(config, field))
                && config.get("auth").and_then(Value::as_str) != Some("none")
                && !has_overrides
            {
                return Err(provider_error(format!(
                    "Provider {provider}: must specify \"baseUrl\", \"headers\", \"apiKey\", \"auth: none\", \"compat\", \"disableStrictTools\", \"guardrailIdentifier\", \"requestMetadata\", \"remoteCompaction\", \"modelOverrides\", \"discovery\", or \"models\""
                )));
            }
        }
    } else {
        if !truthy_field(config, "baseUrl") {
            return Err(provider_error(format!(
                "Provider {provider}: \"baseUrl\" is required when defining custom models."
            )));
        }
        let requires_auth = if mode == ValidationMode::RuntimeRegister {
            !truthy_field(config, "apiKey") && !truthy_field(config, "oauthConfigured")
        } else {
            !truthy_field(config, "apiKey")
                && !matches!(config.get("auth").and_then(Value::as_str).unwrap_or("apiKey"), "none" | "oauth")
        };
        if requires_auth {
            return Err(provider_error(if mode == ValidationMode::RuntimeRegister {
                format!("Provider {provider}: \"apiKey\" or \"oauth\" is required when defining models.")
            } else {
                format!(
                    "Provider {provider}: \"apiKey\" is required when defining custom models unless auth is \"none\" or \"oauth\"."
                )
            }));
        }
    }
    if mode == ValidationMode::ModelsConfig
        && truthy_field(config, "discovery")
        && !has_api
        && config.get("discovery").and_then(|value| value.get("type")).and_then(Value::as_str) != Some("proxy")
    {
        return Err(provider_error(format!(
            "Provider {provider}: \"api\" is required when discovery is enabled at provider level."
        )));
    }
    for model in models {
        let id = property_string(model.get("id"));
        if !has_api && !truthy_field(model, "api") {
            return Err(provider_error(if mode == ValidationMode::RuntimeRegister {
                format!("Provider {provider}, model {id}: no \"api\" specified.")
            } else {
                format!("Provider {provider}, model {id}: no \"api\" specified. Set at provider or model level.")
            }));
        }
        if !truthy_field(model, "id") {
            return Err(provider_error(format!("Provider {provider}: model missing \"id\"")));
        }
        if mode == ValidationMode::ModelsConfig {
            for field in ["contextWindow", "maxTokens"] {
                if model.get(field).is_some_and(|value| numeric(value) <= 0.0) {
                    return Err(provider_error(format!("Provider {provider}, model {id}: invalid {field}")));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema(value: Value) -> Value {
        ModelsConfig::validate_schema(value).unwrap().0
    }
    fn wrap(provider: Value) -> Value {
        json!({"providers":{"fixture":provider}})
    }
    fn custom() -> Value {
        json!({"baseUrl":"https://api.example.invalid/v1","apiKey":"fixture","api":"openai-completions","models":[{"id":"model"}]})
    }
    fn valid(value: Value) -> Value {
        ModelsConfig::validate(value).unwrap().0
    }

    #[test]
    fn models_config_fixed_auth_validation_fixtures() {
        for auth in ["oauth", "none"] {
            let config = json!({"baseUrl":"https://api.example.invalid/v1","auth":auth,"models":[{"id":"grok-4","api":"openai-completions"}]});
            validate_provider_configuration("custom", &config, ValidationMode::ModelsConfig).unwrap();
            valid(wrap(config));
        }
        let mut config = custom();
        config.as_object_mut().unwrap().remove("apiKey");
        let error = validate_provider_configuration("custom", &config, ValidationMode::ModelsConfig).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Provider custom: \"apiKey\" is required when defining custom models unless auth is \"none\" or \"oauth\"."
        );
        config["apiKey"] = json!("fixture");
        config["auth"] = json!("apiKey");
        validate_provider_configuration("custom", &config, ValidationMode::ModelsConfig).unwrap();
    }

    #[test]
    fn models_config_error_stages_and_schema_issue_collection() {
        let error = ModelsConfig::validate(wrap(json!({"api":"invalid","authHeader":"wrong","models":[{"id":null}]})))
            .unwrap_err();
        assert_eq!(error.stage, ConfigValidationStage::Schema);
        assert_eq!(error.issues.len(), 3);
        assert!(error.issues.iter().any(|issue| issue.path == "/providers/fixture/models/0/id"));
        let error = ModelsConfig::validate(wrap(json!({}))).unwrap_err();
        assert_eq!(error.stage, ConfigValidationStage::Provider);
        let error = ModelsConfig::validate_schema(json!({"providers":{"a/b~c":{"apiKey":false}}})).unwrap_err();
        assert_eq!(error.issues[0].path, "/providers/a~1b~0c/apiKey");
    }

    #[test]
    fn models_config_all_api_tokenizer_transport_and_guardrail_values() {
        for api in APIS {
            let mut config = custom();
            config["api"] = json!(api);
            valid(wrap(config));
        }
        for tokenizer in TOKENIZERS {
            let mut config = custom();
            config["models"][0]["tokenizer"] = json!(tokenizer);
            valid(wrap(config));
        }
        let mut config = custom();
        config["transport"] = json!("pi-native");
        config["authHeader"] = json!(false);
        config["guardrailIdentifier"] = json!("");
        config["guardrailVersion"] = json!("");
        config["guardrailTrace"] = json!("enabled_full");
        config["requestMetadata"] = json!({"unvalidated ! key":"arbitrary unicode 界"});
        valid(wrap(config.clone()));
        for trace in ["enabled", "disabled"] {
            config["guardrailTrace"] = json!(trace);
            valid(wrap(config.clone()));
        }
        for field in ["transport", "guardrailTrace", "auth", "api"] {
            let mut invalid = config.clone();
            invalid[field] = json!("not-a-value");
            assert!(ModelsConfig::validate_schema(wrap(invalid)).is_err(), "{field}");
        }
    }

    #[test]
    fn models_config_unknown_fields_and_thinking_pipe_contract() {
        let config = wrap(json!({"auth":"none","providerUnknown":{"null":null},"models":[]}));
        assert_eq!(valid(config.clone()), config);
        let mut config = custom();
        config["models"][0]["unknown"] = json!({"nested":[null, true]});
        config["models"][0]["thinking"] = json!({"mode":"effort","levels":["high","low","high"],"minLevel":"minimal","maxLevel":"max","unknown":true,"defaultLevel":"minimal","supportsDisplay":false,"requiresEffort":true,"effortMap":{"high":"custom","unknown":null}});
        let result = valid(wrap(config));
        assert_eq!(
            result["providers"]["fixture"]["models"][0]["thinking"],
            json!({"mode":"effort","efforts":["high","low","high"],"defaultLevel":"minimal","supportsDisplay":false,"requiresEffort":true,"effortMap":{"high":"custom","unknown":null}})
        );
        assert_eq!(result["providers"]["fixture"]["models"][0]["unknown"], json!({"nested":[null,true]}));
    }

    #[test]
    fn models_config_thinking_precedence_empty_arrays_and_inverse_range() {
        for (thinking, efforts) in [
            (json!({"mode":"effort","efforts":[],"levels":["high"]}), json!([])),
            (json!({"mode":"budget","levels":[],"minLevel":"low","maxLevel":"high"}), json!([])),
            (json!({"mode":"google-level","minLevel":"low","maxLevel":"high"}), json!(["low", "medium", "high"])),
            (json!({"mode":"anthropic-adaptive","minLevel":"high","maxLevel":"low"}), json!(["high"])),
            (
                json!({"mode":"anthropic-budget-effort","efforts":["max","minimal","max"]}),
                json!(["max", "minimal", "max"]),
            ),
        ] {
            let mut config = custom();
            config["models"][0]["thinking"] = thinking;
            assert_eq!(valid(wrap(config))["providers"]["fixture"]["models"][0]["thinking"]["efforts"], efforts);
        }
        for thinking in [
            json!({"mode":"effort"}),
            json!({"mode":"effort","minLevel":"low"}),
            json!({"mode":"effort","efforts":null}),
            json!({"mode":"off","efforts":[]}),
            json!({"mode":"effort","levels":["off"]}),
        ] {
            let mut config = custom();
            config["models"][0]["thinking"] = thinking;
            assert!(ModelsConfig::validate_schema(wrap(config)).is_err());
        }
    }

    #[test]
    fn models_config_omptype_arrays_as_objects_and_morph_spreads() {
        assert_eq!(schema(json!([])), json!({}));
        assert_eq!(schema(json!(["unknown"])), json!({"0":"unknown"}));
        assert_eq!(schema(json!({"providers":[]})), json!({"providers":{}}));
        let result = valid(wrap(
            json!({"headers":[],"compat":[],"remoteCompaction":[],"requestMetadata":[],"modelOverrides":[[]]}),
        ));
        assert_eq!(result["providers"]["fixture"]["headers"], json!([]));
        assert_eq!(result["providers"]["fixture"]["modelOverrides"], json!({"0":{}}));
        assert_eq!(schema(json!({"providers":[[]]})), json!({"providers":{"0":{}}}));
        assert!(ModelsConfig::validate(json!({"providers":[[]]})).is_err());
        assert!(ModelsConfig::validate_schema(wrap(json!({"models":[[]]}))).is_err());
    }

    #[test]
    fn models_config_empty_provider_business_truthiness() {
        for config in [
            json!({"headers":{}}),
            json!({"headers":[]}),
            json!({"compat":{}}),
            json!({"requestMetadata":{}}),
            json!({"remoteCompaction":{}}),
            json!({"modelOverrides":{"id":{}}}),
            json!({"auth":"none"}),
            json!({"disableStrictTools":true}),
            json!({"guardrailIdentifier":"id"}),
        ] {
            valid(wrap(config));
        }
        for config in [
            json!({}),
            json!({"api":"openai-responses"}),
            json!({"auth":"oauth"}),
            json!({"authHeader":true}),
            json!({"disableStrictTools":false}),
            json!({"modelOverrides":{}}),
            json!({"models":[]}),
            json!({"guardrailVersion":"DRAFT"}),
            json!({"guardrailTrace":"enabled"}),
            json!({"transport":"pi-native"}),
            json!({"unknown":true}),
        ] {
            assert_eq!(ModelsConfig::validate(wrap(config)).unwrap_err().stage, ConfigValidationStage::Provider);
        }
    }

    #[test]
    fn models_config_runtime_mode_keeps_its_different_auth_and_limits_contract() {
        let mut config = custom();
        config.as_object_mut().unwrap().remove("apiKey");
        config["auth"] = json!("none");
        assert!(
            validate_provider_configuration("p", &config, ValidationMode::RuntimeRegister)
                .unwrap_err()
                .to_string()
                .contains("\"apiKey\" or \"oauth\"")
        );
        config["oauthConfigured"] = json!(true);
        config["models"][0]["contextWindow"] = json!(-1);
        config["models"][0]["maxTokens"] = json!(0);
        validate_provider_configuration("p", &config, ValidationMode::RuntimeRegister).unwrap();
        assert!(
            validate_provider_configuration("p", &config, ValidationMode::ModelsConfig)
                .unwrap_err()
                .to_string()
                .contains("invalid contextWindow")
        );
        validate_provider_configuration("p", &json!({"models":[]}), ValidationMode::RuntimeRegister).unwrap();
        assert!(validate_provider_configuration("p", &json!({}), ValidationMode::RuntimeRegister).is_err());
    }

    #[test]
    fn models_config_discovery_types_constraints_and_proxy_api_exception() {
        for kind in DISCOVERY_TYPES {
            let config = json!({"api":"openai-completions","discovery":{"type":kind,"timeoutMs":0.5}});
            valid(wrap(config));
        }
        valid(wrap(json!({"discovery":{"type":"proxy"}})));
        for kind in DISCOVERY_TYPES.iter().filter(|kind| **kind != "proxy") {
            assert_eq!(
                ModelsConfig::validate(wrap(json!({"discovery":{"type":kind}}))).unwrap_err().stage,
                ConfigValidationStage::Provider
            );
        }
        valid(wrap(json!({"api":"openai-completions","discovery":{"type":"openai-models-list","injectV1":false}})));
        for discovery in [
            json!({"type":"ollama","injectV1":false}),
            json!({"type":"proxy","timeoutMs":0}),
            json!({"type":"proxy","timeoutMs":-1}),
            json!({"type":"unknown"}),
            json!({"type":"proxy","timeoutMs":null}),
        ] {
            assert!(ModelsConfig::validate_schema(wrap(json!({"discovery":discovery}))).is_err());
        }
    }

    #[test]
    fn models_config_complete_model_and_override_field_surfaces() {
        let mut config = custom();
        let mut definition = json!({"id":"model","name":"Name","api":"google-vertex","baseUrl":"model-url","reasoning":true,"thinking":{"mode":"effort","efforts":["low"]},"input":["text","image"],"imageInputDecoder":"stb","tokenizer":"glm5","supportsTools":false,"cost":{"input":-1,"output":0,"cacheRead":0.5,"cacheWrite":2},"premiumMultiplier":-2,"contextWindow":1.5,"maxTokens":2.5,"omitMaxOutputTokens":true,"preferWebsockets":false,"headers":{"x":"v"},"compat":{"promptCacheMode":"explicit"},"contextPromotionTarget":"p/m","compactionModel":"p/c","remoteCompaction":{"endpoint":"endpoint"}});
        config["models"][0] = definition.clone();
        valid(wrap(config.clone()));
        definition.as_object_mut().unwrap().remove("cost");
        definition["cost"] = json!({"input":-1});
        definition["contextWindow"] = json!(0);
        definition["maxTokens"] = json!(-1);
        definition["api"] = Value::Null;
        definition["baseUrl"] = json!(""); // undeclared override properties
        config["modelOverrides"] = json!({"model":definition});
        valid(wrap(config));
        let mut config = custom();
        config["models"][0]["cost"] = json!({"input":1});
        assert_eq!(ModelsConfig::validate_schema(wrap(config)).unwrap_err().issues.len(), 3);
    }

    #[test]
    fn models_config_nonempty_strings_and_nonpositive_definition_limits() {
        for field in ["id", "name", "baseUrl", "contextPromotionTarget", "compactionModel"] {
            let mut config = custom();
            config["models"][0][field] = json!("");
            assert_eq!(
                ModelsConfig::validate(wrap(config)).unwrap_err().stage,
                ConfigValidationStage::Schema,
                "{field}"
            );
        }
        for field in ["baseUrl", "apiKey"] {
            let mut config = custom();
            config[field] = json!("");
            assert!(ModelsConfig::validate_schema(wrap(config)).is_err());
        }
        for field in ["contextWindow", "maxTokens"] {
            for number in [0, -1] {
                let mut config = custom();
                config["models"][0][field] = json!(number);
                assert_eq!(ModelsConfig::validate(wrap(config)).unwrap_err().stage, ConfigValidationStage::Provider);
            }
        }
        let mut config = custom();
        config["models"][0]["id"] = json!(" ");
        config["models"][0]["name"] = json!(" ");
        valid(wrap(config));
    }

    #[test]
    fn models_config_all_compat_booleans_and_conditional_field_contract() {
        for field in OPENAI_COMPAT_BOOLEANS {
            let mut compat = Map::new();
            compat.insert((*field).into(), json!(true));
            valid(wrap(json!({"compat":compat})));
            let mut invalid = Map::new();
            invalid.insert((*field).into(), json!("true"));
            assert!(ModelsConfig::validate_schema(wrap(json!({"compat":invalid}))).is_err(), "{field}");
            let mut invalid = Map::new();
            invalid.insert((*field).into(), Value::Null);
            assert!(
                ModelsConfig::validate_schema(wrap(json!({"compat":{"whenThinking":invalid}}))).is_err(),
                "{field}"
            );
        }
        valid(wrap(json!({"compat":{"whenThinking":{"whenThinking":null,"promptCacheMode":false,"unknown":null}}})));
    }

    #[test]
    fn models_config_remaining_compat_fields_and_bedrock_numeric_constraints() {
        let compat = json!({"reasoningEffortMap":{"minimal":"mi","low":"l","medium":"m","high":"h","xhigh":"x","max":"mx"},"maxTokensField":"max_tokens","reasoningContentField":"reasoning_text","thinkingFormat":"qwen-chat-template","openRouterRouting":{"only":[],"order":["provider"],"unknown":null},"vercelGatewayRouting":{"only":["a"],"order":[]},"extraBody":{"any":null,"nested":{"data":[1,2]}},"cacheControlFormat":"anthropic","toolStrictMode":"none","streamIdleTimeoutMs":0.5,"streamMarkupHealingPattern":"dsml","whenThinking":{"supportsStore":true},"promptCacheMode":"automatic","promptCacheMinimumTokens":0,"promptCacheMaximumCheckpoints":1.5});
        valid(wrap(json!({"compat":compat.clone()})));
        for field in ["streamIdleTimeoutMs", "promptCacheMinimumTokens", "promptCacheMaximumCheckpoints"] {
            let mut invalid = compat.clone();
            invalid[field] = json!(-0.1);
            assert!(ModelsConfig::validate_schema(wrap(json!({"compat":invalid}))).is_err());
        }
        for (field, invalid_value) in [
            ("reasoningEffortMap", json!({"high":1})),
            ("maxTokensField", json!("bad")),
            ("reasoningContentField", json!("bad")),
            ("thinkingFormat", json!("bad")),
            ("openRouterRouting", json!({"only":[1]})),
            ("vercelGatewayRouting", json!({"order":false})),
            ("extraBody", json!(false)),
            ("cacheControlFormat", json!("bad")),
            ("toolStrictMode", json!("bad")),
            ("streamMarkupHealingPattern", json!("bad")),
            ("promptCacheMode", json!("bad")),
        ] {
            let mut invalid = compat.clone();
            invalid[field] = invalid_value;
            assert!(ModelsConfig::validate_schema(wrap(json!({"compat":invalid}))).is_err(), "{field}");
        }
    }

    #[test]
    fn models_config_remote_compaction_complete_surface_and_narrowing() {
        let remote = json!({"enabled":false,"api":"openai-codex-responses","endpoint":"e","model":"m","v2StreamingEnabled":true,"v2Endpoint":"v2","streamingEndpoint":"s","unknown":null});
        valid(wrap(json!({"remoteCompaction":remote.clone()})));
        for field in ["endpoint", "model", "v2Endpoint", "streamingEndpoint"] {
            let mut invalid = remote.clone();
            invalid[field] = json!("");
            assert!(ModelsConfig::validate_schema(wrap(json!({"remoteCompaction":invalid}))).is_err(), "{field}");
        }
        let mut config = custom();
        config["models"][0]["remoteCompaction"] = remote.clone();
        config["modelOverrides"] = json!({"model":{"remoteCompaction":remote}});
        valid(wrap(config));
    }

    #[test]
    fn models_config_null_and_type_constraints_cover_each_nested_shape() {
        for input in [Value::Null, json!(false), json!(1), json!("object")] {
            assert!(ModelsConfig::validate_schema(input).is_err());
        }
        assert!(ModelsConfig::validate_schema(json!({"providers":null})).is_err());
        for field in [
            "baseUrl",
            "apiKey",
            "api",
            "headers",
            "compat",
            "remoteCompaction",
            "authHeader",
            "auth",
            "discovery",
            "models",
            "modelOverrides",
            "disableStrictTools",
            "guardrailIdentifier",
            "guardrailVersion",
            "guardrailTrace",
            "requestMetadata",
            "transport",
        ] {
            let mut config = Map::new();
            config.insert(field.into(), Value::Null);
            assert!(ModelsConfig::validate_schema(wrap(Value::Object(config))).is_err(), "{field}");
        }
        for field in [
            "id",
            "name",
            "api",
            "baseUrl",
            "reasoning",
            "thinking",
            "input",
            "imageInputDecoder",
            "tokenizer",
            "supportsTools",
            "cost",
            "premiumMultiplier",
            "contextWindow",
            "maxTokens",
            "omitMaxOutputTokens",
            "preferWebsockets",
            "headers",
            "compat",
            "contextPromotionTarget",
            "compactionModel",
            "remoteCompaction",
        ] {
            let mut config = custom();
            config["models"][0][field] = Value::Null;
            assert!(ModelsConfig::validate_schema(wrap(config)).is_err(), "{field}");
        }
        for number in [json!(-1), json!(0), json!(0.5)] {
            let mut config = custom();
            config["models"][0]["premiumMultiplier"] = number;
            schema(wrap(config));
        }
        assert!(serde_json::from_str::<Value>("1e9999").is_err());
    }

    #[test]
    fn models_config_business_error_precedence_and_model_level_api() {
        let error = validate_provider_configuration("p", &json!({"models":[{"id":""}]}), ValidationMode::ModelsConfig)
            .unwrap_err();
        assert!(error.to_string().contains("\"baseUrl\" is required"));
        let error = validate_provider_configuration(
            "p",
            &json!({"baseUrl":"url","auth":"none","models":[{"id":""}]}),
            ValidationMode::ModelsConfig,
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "Provider p, model : no \"api\" specified. Set at provider or model level.");
        let error = validate_provider_configuration(
            "p",
            &json!({"baseUrl":"url","auth":"none","api":"api","models":[{"id":""}]}),
            ValidationMode::ModelsConfig,
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "Provider p: model missing \"id\"");
        let mut config = custom();
        config.as_object_mut().unwrap().remove("api");
        config["models"][0]["api"] = json!("anthropic-messages");
        valid(wrap(config));
    }
}
