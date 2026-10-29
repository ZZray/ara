//! Portable OpenAI thinking inputs and baked model policy.
//!
//! Fixed OMP 596f2da7101178214aa27a753529d15e6b7ad91d:
//! catalog/src/model-thinking.ts; ai/src/stream.ts:1879-1963;
//! providers/openai-shared.ts:846-1196,3618-3695;
//! providers/openai-codex/request-transformer.ts:130-169,451-482.
//! MIT; see THIRD_PARTY_NOTICES.md. No identity or URL inference is performed.

use crate::providers::openai_completions::{MaxTokensField, OpenAICompat, ToolSchemaFlavor, ToolStrictMode};
use crate::{Model, ProviderError};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
    Max,
}

impl ReasoningEffort {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "minimal" => Some(Self::Minimal),
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "xhigh" => Some(Self::XHigh),
            "max" => Some(Self::Max),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReasoningSummary {
    #[default]
    ProviderDefault,
    Hidden,
    Auto,
    Concise,
    Detailed,
}

impl ReasoningSummary {
    fn wire(self) -> Option<&'static str> {
        match self {
            Self::ProviderDefault | Self::Hidden => None,
            Self::Auto => Some("auto"),
            Self::Concise => Some("concise"),
            Self::Detailed => Some("detailed"),
        }
    }
}

/// One per-call replacement. `None` on CallOptions inherits the route base;
/// `Some(Default::default())` clears its effort, disable and summary controls.
/// Auto, inheritance selectors and Session ceilings belong to the Host.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReasoningRequest {
    pub effort: Option<ReasoningEffort>,
    pub disable_reasoning: bool,
    pub force_reasoning_off: bool,
    pub summary: ReasoningSummary,
    pub thinking_budgets: BTreeMap<ReasoningEffort, u64>,
}

/// Keys consumed by the materialized-model projection. Other base compat
/// fields remain subject to the Host's existing protocol preparation checks.
pub const OPENAI_REASONING_COMPAT_KEYS: &[&str] = &[
    "supportsReasoningParams",
    "supportsReasoningEffort",
    "supportsReasoningSummary",
    "reasoningEffortMap",
    "omitReasoningEffort",
    "reasoningDisableMode",
    "thinkingFormat",
    "thinkingKeep",
    "whenThinking",
    "supportsToolChoice",
    "supportsForcedToolChoice",
    "supportsNamedToolChoice",
    "supportsImageDetailOriginal",
    "disableReasoningOnForcedToolChoice",
    "disableReasoningOnToolChoice",
    "qwenPreserveThinking",
    "qwenTemplateReasoningEffort",
    "includeEncryptedReasoning",
    "requiresReasoningOffJuiceInstruction",
    "isOpenRouterHost",
    "supportsStrictMode",
    "toolStrictMode",
    "toolSchemaFlavor",
    "rejectRootObjectUnion",
    "retryWithoutStrictOnGrammarError",
];

const MESSAGE_KEYS: &[&str] = &[
    "supportsDeveloperRole",
    "supportsUsageInStreaming",
    "supportsImages",
    "maxTokensField",
    "requiresReasoningContentForAllAssistantTurns",
    "requiresMistralToolIds",
    "requiresToolResultName",
    "requiresAssistantAfterToolResult",
    "requiresThinkingAsText",
    "supportsMultipleSystemMessages",
    "usesOpenAIToolCallIdLimit",
    "reasoningContentField",
    "requiresReasoningContentForToolCalls",
    "allowsSyntheticReasoningContentForToolCalls",
    "replayReasoningContent",
    "requiresAssistantContentForToolCalls",
    "reasoningDeltasMayBeCumulative",
    "stripDeepseekSpecialTokens",
    "streamMarkupHealingPattern",
    "emptyLengthFinishIsContextError",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DisableMode {
    Omit,
    Lowest,
    None,
    OpenRouter,
    Cline,
    Venice,
    Zai,
    Qwen,
    QwenTemplate,
    ChatTemplate,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct MessageCompatPatch {
    developer: Option<bool>,
    usage: Option<bool>,
    images: Option<bool>,
    max_tokens: Option<MaxTokensField>,
    reasoning_content: Option<bool>,
    mistral_ids: Option<bool>,
    result_name: Option<bool>,
    assistant_after_result: Option<bool>,
    thinking_text: Option<bool>,
    multiple_system: Option<bool>,
    tool_id_limit: Option<bool>,
    thinking_format: Option<String>,
    reasoning_field: Option<String>,
    reasoning_for_calls: Option<bool>,
    synthetic_reasoning: Option<bool>,
    replay_reasoning: Option<bool>,
    assistant_for_calls: Option<bool>,
    cumulative_reasoning: Option<bool>,
    strip_deepseek: Option<bool>,
    markup: Option<Option<crate::providers::openai_completions::actual::markup::HealingPattern>>,
    empty_length_error: Option<bool>,
}

impl MessageCompatPatch {
    fn parse(value: &Value) -> Result<Self, ProviderError> {
        Ok(Self {
            developer: bool_option(value, "supportsDeveloperRole")?,
            usage: bool_option(value, "supportsUsageInStreaming")?,
            images: bool_option(value, "supportsImages")?,
            max_tokens: match string_option(value, "maxTokensField")? {
                None => None,
                Some("max_tokens") => Some(MaxTokensField::MaxTokens),
                Some("max_completion_tokens") => Some(MaxTokensField::MaxCompletionTokens),
                Some(_) => return Err(invalid("compat.maxTokensField", &value["maxTokensField"])),
            },
            reasoning_content: bool_option(value, "requiresReasoningContentForAllAssistantTurns")?,
            mistral_ids: bool_option(value, "requiresMistralToolIds")?,
            result_name: bool_option(value, "requiresToolResultName")?,
            assistant_after_result: bool_option(value, "requiresAssistantAfterToolResult")?,
            thinking_text: bool_option(value, "requiresThinkingAsText")?,
            multiple_system: bool_option(value, "supportsMultipleSystemMessages")?,
            tool_id_limit: bool_option(value, "usesOpenAIToolCallIdLimit")?,
            thinking_format: string_option(value, "thinkingFormat")?.map(str::to_owned),
            reasoning_field: match string_option(value, "reasoningContentField")? {
                Some(field @ ("reasoning_content" | "reasoning" | "reasoning_text")) => Some(field.into()),
                None => None,
                Some(_) => return Err(invalid("compat.reasoningContentField", &value["reasoningContentField"])),
            },
            reasoning_for_calls: bool_option(value, "requiresReasoningContentForToolCalls")?,
            synthetic_reasoning: bool_option(value, "allowsSyntheticReasoningContentForToolCalls")?,
            replay_reasoning: bool_option(value, "replayReasoningContent")?,
            assistant_for_calls: bool_option(value, "requiresAssistantContentForToolCalls")?,
            cumulative_reasoning: bool_option(value, "reasoningDeltasMayBeCumulative")?,
            strip_deepseek: bool_option(value, "stripDeepseekSpecialTokens")?,
            empty_length_error: bool_option(value, "emptyLengthFinishIsContextError")?,
            markup: match value.get("streamMarkupHealingPattern") {
                None => None,
                Some(Value::Null) => Some(None),
                Some(value) => {
                    use crate::providers::openai_completions::actual::markup::HealingPattern as P;
                    Some(Some(match value.as_str() {
                        Some("kimi") => P::Kimi,
                        Some("dsml") => P::Dsml,
                        Some("qwen") => P::Qwen,
                        Some("thinking") => P::Thinking,
                        _ => return Err(invalid("compat.streamMarkupHealingPattern", value)),
                    }))
                }
            },
        })
    }

    fn apply(&self, base: &OpenAICompat) -> OpenAICompat {
        let mut result = base.clone();
        macro_rules! apply {
            ($field:ident, $patch:ident) => {
                if let Some(value) = &self.$patch {
                    result.$field = *value;
                }
            };
        }
        apply!(supports_developer_role, developer);
        apply!(supports_usage_in_streaming, usage);
        apply!(supports_images, images);
        apply!(max_tokens_field, max_tokens);
        apply!(requires_reasoning_content_on_all_assistant_turns, reasoning_content);
        apply!(requires_mistral_tool_ids, mistral_ids);
        apply!(requires_tool_result_name, result_name);
        apply!(requires_assistant_after_tool_result, assistant_after_result);
        apply!(requires_thinking_as_text, thinking_text);
        apply!(supports_multiple_system_messages, multiple_system);
        apply!(uses_openai_tool_call_id_limit, tool_id_limit);
        apply!(requires_reasoning_content_for_tool_calls, reasoning_for_calls);
        apply!(allows_synthetic_reasoning_content_for_tool_calls, synthetic_reasoning);
        apply!(replay_reasoning_content, replay_reasoning);
        apply!(requires_assistant_content_for_tool_calls, assistant_for_calls);
        apply!(reasoning_deltas_may_be_cumulative, cumulative_reasoning);
        apply!(strip_deepseek_special_tokens, strip_deepseek);
        apply!(empty_length_finish_is_context_error, empty_length_error);
        if let Some(pattern) = self.markup {
            result.stream_markup_healing_pattern = pattern;
        }
        if let Some(value) = &self.thinking_format {
            result.thinking_format = value.clone();
        }
        if let Some(value) = &self.reasoning_field {
            result.reasoning_content_field = Some(value.clone());
        }
        result
    }
}

#[derive(Clone, Debug, PartialEq)]
struct ReasoningCompat {
    params: bool,
    effort: bool,
    summary: bool,
    omit: bool,
    disable: DisableMode,
    dialect: String,
    map: BTreeMap<ReasoningEffort, String>,
    keep: Option<Value>,
    tools: bool,
    forced: bool,
    named: bool,
    original_image_detail: bool,
    suppress_forced: bool,
    suppress_any: bool,
    qwen_preserve: bool,
    qwen_effort: bool,
    encrypted: bool,
    off_juice: bool,
    openrouter: bool,
    messages: MessageCompatPatch,
    chat_schema: NativeChatSchemaPolicy,
}

impl ReasoningCompat {
    fn parse(value: &Value) -> Result<Self, ProviderError> {
        let dialect = string_option(value, "thinkingFormat")?.unwrap_or("openai");
        if !matches!(
            dialect,
            "openai" | "openrouter" | "zai" | "kimi" | "qwen" | "qwen-chat-template" | "chat-template"
        ) {
            return Err(invalid("compat.thinkingFormat", &value["thinkingFormat"]));
        }
        let disable = match string_option(value, "reasoningDisableMode")?.unwrap_or("omit") {
            "omit" => DisableMode::Omit,
            "lowest-effort" => DisableMode::Lowest,
            "none-effort" => DisableMode::None,
            "openrouter-enabled-false" => DisableMode::OpenRouter,
            "cline-enabled-false" => DisableMode::Cline,
            "venice-disable-thinking" => DisableMode::Venice,
            "zai-thinking-disabled" => DisableMode::Zai,
            "qwen-enable-thinking-false" => DisableMode::Qwen,
            "qwen-template-false" => DisableMode::QwenTemplate,
            "chat-template-thinking-false" => DisableMode::ChatTemplate,
            _ => return Err(invalid("compat.reasoningDisableMode", &value["reasoningDisableMode"])),
        };
        let keep = value.get("thinkingKeep").filter(|v| !v.is_null()).cloned();
        if keep.as_ref().is_some_and(|v| v != "all" && v != &Value::Bool(false)) {
            return Err(invalid("compat.thinkingKeep", &value["thinkingKeep"]));
        }
        Ok(Self {
            params: bool_value(value, "supportsReasoningParams", true)?,
            effort: bool_value(value, "supportsReasoningEffort", true)?,
            summary: bool_value(value, "supportsReasoningSummary", true)?,
            omit: bool_value(value, "omitReasoningEffort", false)?,
            disable,
            dialect: dialect.into(),
            map: effort_map(value.get("reasoningEffortMap"), "compat.reasoningEffortMap")?,
            keep,
            tools: bool_value(value, "supportsToolChoice", true)?,
            forced: bool_value(value, "supportsForcedToolChoice", true)?,
            named: bool_value(value, "supportsNamedToolChoice", true)?,
            original_image_detail: bool_value(value, "supportsImageDetailOriginal", false)?,
            suppress_forced: bool_value(value, "disableReasoningOnForcedToolChoice", false)?,
            suppress_any: bool_value(value, "disableReasoningOnToolChoice", false)?,
            qwen_preserve: bool_value(value, "qwenPreserveThinking", false)?,
            qwen_effort: bool_value(value, "qwenTemplateReasoningEffort", false)?,
            encrypted: bool_value(value, "includeEncryptedReasoning", true)?,
            off_juice: bool_value(value, "requiresReasoningOffJuiceInstruction", false)?,
            openrouter: bool_value(value, "isOpenRouterHost", false)?,
            messages: MessageCompatPatch::parse(value)?,
            chat_schema: NativeChatSchemaPolicy::parse(value)?,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
struct ThinkingPolicy {
    efforts: Vec<ReasoningEffort>,
    default: Option<ReasoningEffort>,
    map: BTreeMap<ReasoningEffort, String>,
    budgets: BTreeMap<ReasoningEffort, u64>,
    routing: BTreeMap<String, String>,
    budget: bool,
    required: bool,
    suppress_off: bool,
}

/// A projection of the selected HostModel.spec(), without Host state. Model
/// identity is retained by the caller; requestModelId/effortRouting alter only
/// the outbound body. No model-name ladder, Auto classification or ceiling is invented.
#[derive(Clone, Debug, PartialEq)]
pub struct OpenAIReasoningPolicy {
    supported: bool,
    thinking: Option<ThinkingPolicy>,
    compat: ReasoningCompat,
    when_thinking: Option<ReasoningCompat>,
    request_model_id: Option<String>,
    mode: Option<String>,
    history_prefix_binding: bool,
}

impl OpenAIReasoningPolicy {
    /// The selected Host model's baked signature contract. The provider owns
    /// request projection; Host still owns model selection and Session state.
    pub(crate) fn history_prefix_binding(&self) -> bool {
        self.history_prefix_binding
    }
    pub(crate) fn supports_forced_tool_choice(&self) -> bool {
        self.compat.forced
    }
    pub(crate) fn supports_original_image_detail(&self) -> bool {
        self.compat.original_image_detail
    }
    pub fn from_model_spec(spec: &Value) -> Result<Self, ProviderError> {
        if !spec.is_object() {
            return Err(invalid("model", spec));
        }
        let empty = json!({});
        let compat = spec.get("compat").filter(|v| !v.is_null()).unwrap_or(&empty);
        if !compat.is_object() {
            return Err(invalid("compat", compat));
        }
        let base = ReasoningCompat::parse(compat)?;
        let when_thinking = match compat.get("whenThinking").filter(|v| !v.is_null()) {
            None => None,
            Some(value) => {
                let values = value.as_object().ok_or_else(|| invalid("compat.whenThinking", value))?;
                let mut merged = compat.as_object().unwrap().clone();
                merged.remove("whenThinking");
                for (key, value) in values {
                    if key == "whenThinking"
                        || (!OPENAI_REASONING_COMPAT_KEYS.contains(&key.as_str())
                            && !MESSAGE_KEYS.contains(&key.as_str())
                            && compat.get(key) != Some(value))
                    {
                        return Err(invalid(&format!("compat.whenThinking.{key}"), value));
                    }
                    merged.insert(key.clone(), value.clone());
                }
                Some(ReasoningCompat::parse(&Value::Object(merged))?)
            }
        };
        let thinking = match spec.get("thinking").filter(|v| !v.is_null()) {
            None => None,
            Some(value) => {
                let values = value.as_object().ok_or_else(|| invalid("thinking", value))?;
                for (key, field) in values {
                    if ![
                        "mode",
                        "efforts",
                        "defaultLevel",
                        "effortMap",
                        "effortBudgets",
                        "effortRouting",
                        "requiresEffort",
                        "suppressWhenOff",
                        "prefixBinding",
                        "supportsDisplay",
                    ]
                    .contains(&key.as_str())
                    {
                        return Err(invalid(&format!("thinking.{key}"), field));
                    }
                }
                // These baked capabilities stay on the original Host model.
                // Host reduction consumes prefixBinding; supportsDisplay is
                // another transport's capability, not an OpenAI request knob.
                bool_option(value, "prefixBinding")?;
                bool_option(value, "supportsDisplay")?;
                let mode = string_option(value, "mode")?.ok_or_else(|| invalid("thinking.mode", &value["mode"]))?;
                if !matches!(mode, "effort" | "budget") {
                    return Err(invalid("thinking.mode", &value["mode"]));
                }
                let raw = value
                    .get("efforts")
                    .and_then(Value::as_array)
                    .ok_or_else(|| invalid("thinking.efforts", &value["efforts"]))?;
                let efforts = raw.iter().map(|v| parse_effort(v, "thinking.efforts")).collect::<Result<Vec<_>, _>>()?;
                if efforts.is_empty() || efforts.windows(2).any(|pair| pair[0] >= pair[1]) {
                    return Err(invalid("thinking.efforts", &value["efforts"]));
                }
                let default = value
                    .get("defaultLevel")
                    .filter(|v| !v.is_null())
                    .map(|v| parse_effort(v, "thinking.defaultLevel"))
                    .transpose()?;
                if default.is_some_and(|effort| !efforts.contains(&effort)) {
                    return Err(invalid("thinking.defaultLevel", &value["defaultLevel"]));
                }
                let mut routing = BTreeMap::new();
                if let Some(map) = value.get("effortRouting").filter(|v| !v.is_null()) {
                    for (key, wire) in map.as_object().ok_or_else(|| invalid("thinking.effortRouting", map))? {
                        if key != "off" {
                            parse_effort(&json!(key), "thinking.effortRouting")?;
                        }
                        let wire = wire
                            .as_str()
                            .filter(|s| !s.is_empty())
                            .ok_or_else(|| invalid(&format!("thinking.effortRouting.{key}"), wire))?;
                        routing.insert(key.clone(), wire.into());
                    }
                }
                Some(ThinkingPolicy {
                    efforts,
                    default,
                    map: effort_map(value.get("effortMap"), "thinking.effortMap")?,
                    budgets: budget_map(value.get("effortBudgets"), "thinking.effortBudgets")?,
                    routing,
                    budget: mode == "budget",
                    required: bool_value(value, "requiresEffort", false)?,
                    suppress_off: bool_value(value, "suppressWhenOff", false)?,
                })
            }
        };
        let mode = string_option(spec, "reasoningMode")?;
        if mode.is_some_and(|mode| mode != "pro") {
            return Err(invalid("reasoningMode", &spec["reasoningMode"]));
        }
        Ok(Self {
            supported: bool_value(spec, "reasoning", false)?,
            thinking,
            compat: base,
            when_thinking,
            request_model_id: string_option(spec, "requestModelId")?.map(str::to_owned),
            mode: mode.map(str::to_owned),
            history_prefix_binding: match spec.get("thinking").filter(|value| !value.is_null()) {
                Some(value) => bool_value(value, "prefixBinding", false)?,
                None => false,
            },
        })
    }

    pub fn supported_efforts(&self) -> &[ReasoningEffort] {
        if self.supported { self.thinking.as_ref().map(|t| t.efforts.as_slice()).unwrap_or(&[]) } else { &[] }
    }
    pub fn default_effort(&self) -> Option<ReasoningEffort> {
        self.thinking.as_ref().and_then(|t| t.default)
    }
    pub fn clamp_effort(&self, requested: ReasoningEffort) -> Option<ReasoningEffort> {
        let efforts = self.supported_efforts();
        efforts.iter().rev().find(|effort| **effort <= requested).or_else(|| efforts.first()).copied()
    }
    /// Fixed Native modelSupportsEffortCeiling query. This only checks the
    /// ladder, not the actual effort chosen by a mandatory default route.
    pub fn model_supports_effort_ceiling(&self, ceiling: ReasoningEffort) -> bool {
        let efforts = self.supported_efforts();
        efforts.is_empty() || efforts.iter().any(|effort| *effort <= ceiling)
    }
    pub fn default_supported_effort(&self) -> Option<ReasoningEffort> {
        if let (Some(thinking), Some(default)) = (&self.thinking, &self.request_model_id)
            && let Some(effort) =
                self.supported_efforts().iter().find(|e| thinking.routing.get(e.as_str()) == Some(default))
        {
            return Some(*effort);
        }
        self.supported_efforts().first().copied()
    }
    /// Pure provider-boundary normalization, including mandatory defaults.
    /// Hosts can inspect the resulting effort before admitting a request
    /// under their own ceiling. This does not apply or clamp a Host ceiling.
    pub fn normalize_request(&self, request: Option<&ReasoningRequest>) -> Result<ReasoningRequest, ProviderError> {
        self.normalize(request)
    }
    fn map_effort(&self, compat: &ReasoningCompat, effort: ReasoningEffort) -> String {
        compat
            .map
            .get(&effort)
            .or_else(|| self.thinking.as_ref().and_then(|t| t.map.get(&effort)))
            .cloned()
            .unwrap_or_else(|| effort.as_str().into())
    }
    fn normalize(&self, request: Option<&ReasoningRequest>) -> Result<ReasoningRequest, ProviderError> {
        let mut request = request.cloned().unwrap_or_default();
        if self.supported
            && self.thinking.as_ref().is_some_and(|t| t.required && !t.suppress_off)
            && (request.effort.is_none() || request.disable_reasoning || request.force_reasoning_off)
            && let Some(floor) = self.default_supported_effort()
        {
            request.effort = Some(floor);
            request.disable_reasoning = false;
            request.force_reasoning_off = false;
        }
        if let Some(effort) = request.effort {
            if !self.supported || self.thinking.is_none() {
                request.effort = None;
            } else if !self.supported_efforts().contains(&effort) {
                if let Some(mapped) = self
                    .compat
                    .map
                    .get(&effort)
                    .and_then(|s| serde_json::from_value::<ReasoningEffort>(json!(s)).ok())
                    .filter(|e| self.supported_efforts().contains(e))
                {
                    request.effort = Some(mapped);
                } else if !self.compat.map.contains_key(&effort)
                    && !self.thinking.as_ref().is_some_and(|t| t.map.contains_key(&effort))
                {
                    return Err(ProviderError::Config(format!(
                        "Thinking effort {} is not supported by this model policy",
                        effort.as_str()
                    )));
                }
            }
        }
        Ok(request)
    }
    fn wire_model_id<'a>(&'a self, model: &'a Model, effort: Option<ReasoningEffort>) -> &'a str {
        self.thinking
            .as_ref()
            .and_then(|t| t.routing.get(effort.map(ReasoningEffort::as_str).unwrap_or("off")))
            .or(self.request_model_id.as_ref())
            .map(String::as_str)
            .unwrap_or(&model.id)
    }
}

struct Resolved<'a> {
    compat: &'a ReasoningCompat,
    request: ReasoningRequest,
    wire: Option<String>,
    enabled: bool,
    disabled: bool,
    caller: bool,
    forced_conflict: bool,
}

fn resolve<'a>(
    policy: &'a OpenAIReasoningPolicy,
    request: Option<&ReasoningRequest>,
    tool: Option<&Value>,
) -> Result<Resolved<'a>, ProviderError> {
    resolve_compat(policy, &policy.compat, policy.when_thinking.as_ref(), request, tool)
}

fn resolve_compat<'a>(
    policy: &'a OpenAIReasoningPolicy,
    base: &'a ReasoningCompat,
    when: Option<&'a ReasoningCompat>,
    request: Option<&ReasoningRequest>,
    tool: Option<&Value>,
) -> Result<Resolved<'a>, ProviderError> {
    let request = policy.normalize(request)?;
    let forced = base.forced && tool.is_some_and(|v| v == "required" || v.is_object());
    let conflict = (base.suppress_forced && forced) || (base.suppress_any && tool.is_some());
    let allowed = request.effort.is_some() && !request.disable_reasoning && policy.supported && !conflict;
    let mapped_none = allowed
        && base.disable == DisableMode::Zai
        && request.effort.is_some_and(|e| policy.map_effort(base, e) == "none");
    let enabled = allowed && !mapped_none;
    let compat = if enabled { when.unwrap_or(base) } else { base };
    let implicit = policy.supported
        && request.effort.is_none()
        && !request.disable_reasoning
        && matches!(
            compat.disable,
            DisableMode::Zai | DisableMode::Qwen | DisableMode::QwenTemplate | DisableMode::ChatTemplate
        );
    let disabled = (policy.supported && request.disable_reasoning) || conflict || implicit || mapped_none;
    let mut wire = if enabled { request.effort.map(|e| policy.map_effort(compat, e)) } else { None };
    if disabled && compat.effort && !compat.omit {
        if compat.disable == DisableMode::None {
            wire = Some("none".into());
        } else if request.disable_reasoning && request.effort.is_none() && compat.disable == DisableMode::Lowest {
            let floor = policy
                .supported_efforts()
                .first()
                .copied()
                .ok_or_else(|| ProviderError::Config("Model policy has no supported reasoning efforts".into()))?;
            wire = Some(policy.map_effort(compat, floor));
        }
    }
    let caller = request.disable_reasoning;
    Ok(Resolved {
        compat,
        request,
        wire,
        enabled,
        disabled,
        caller,
        forced_conflict: !caller && base.suppress_forced && forced,
    })
}

fn policy_required<'a>(
    request: Option<&ReasoningRequest>,
    policy: Option<&'a OpenAIReasoningPolicy>,
) -> Result<Option<&'a OpenAIReasoningPolicy>, ProviderError> {
    if request.is_some() && policy.is_none() {
        return Err(ProviderError::Config("Reasoning request requires the selected materialized model policy".into()));
    }
    Ok(policy)
}

/// Operates on the tool choice that survived actual tool/schema preparation.
pub(crate) fn prepare_tool_choice(
    body: &mut Value,
    policy: &OpenAIReasoningPolicy,
    request: Option<&ReasoningRequest>,
) -> Result<(), ProviderError> {
    let request = policy.normalize(request)?;
    if !policy.compat.tools {
        body.as_object_mut().unwrap().remove("tool_choice");
        return Ok(());
    }
    if !policy.compat.named
        && body.get("tool_choice").is_some_and(|choice| choice.get("type").and_then(Value::as_str) == Some("function"))
    {
        let name = body["tool_choice"]
            .get("function")
            .and_then(|v| v.get("name"))
            .or_else(|| body["tool_choice"].get("name"))
            .cloned();
        if let (Some(name), Some(tools)) = (name, body.get_mut("tools").and_then(Value::as_array_mut)) {
            tools.retain(|tool| {
                tool.get("function").and_then(|v| v.get("name")).or_else(|| tool.get("name")) == Some(&name)
            });
        }
        body["tool_choice"] = json!("required");
    }
    if !policy.compat.forced && body.get("tool_choice").is_some_and(|v| v == "required" || v == "any" || v.is_object())
    {
        body["tool_choice"] = json!("auto");
    }
    if policy.compat.suppress_any
        && body.get("tool_choice").is_some_and(|v| v == "auto")
        && policy.supported
        && request.effort.is_some()
        && !request.disable_reasoning
    {
        body.as_object_mut().unwrap().remove("tool_choice");
    }
    Ok(())
}

/// Fixed Chat buildParams resolves requested choice before preparing Native
/// tools, then resolves again from the surviving choice. Only materialized
/// policy controls cross this boundary; original tools/schema/history do not.
pub(crate) struct NativeChatInitialPolicy {
    pub compat: OpenAICompat,
    pub supports_tool_choice: bool,
    pub supports_forced_tool_choice: bool,
    pub supports_named_tool_choice: bool,
    pub drop_auto_for_reasoning: bool,
    pub schema: NativeChatSchemaPolicy,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct NativeChatSchemaPolicy {
    pub supports_strict_mode: bool,
    pub strict_mode: ToolStrictMode,
    pub flavor: ToolSchemaFlavor,
    pub reject_root_union: bool,
    pub retry_on_grammar_error: bool,
}
impl Default for NativeChatSchemaPolicy {
    fn default() -> Self {
        Self {
            supports_strict_mode: true,
            strict_mode: ToolStrictMode::Mixed,
            flavor: ToolSchemaFlavor::None,
            reject_root_union: false,
            retry_on_grammar_error: false,
        }
    }
}
impl NativeChatSchemaPolicy {
    fn parse(value: &Value) -> Result<Self, ProviderError> {
        let strict_mode = match string_option(value, "toolStrictMode")?.unwrap_or("mixed") {
            "mixed" => ToolStrictMode::Mixed,
            "all_strict" => ToolStrictMode::AllStrict,
            "none" => ToolStrictMode::None,
            _ => return Err(invalid("compat.toolStrictMode", &value["toolStrictMode"])),
        };
        let flavor = match string_option(value, "toolSchemaFlavor")?.unwrap_or("none") {
            "none" => ToolSchemaFlavor::None,
            "grammar" => ToolSchemaFlavor::Grammar,
            "moonshot-mfjs" => ToolSchemaFlavor::MoonshotMfjs,
            _ => return Err(invalid("compat.toolSchemaFlavor", &value["toolSchemaFlavor"])),
        };
        Ok(Self {
            supports_strict_mode: bool_value(value, "supportsStrictMode", true)?,
            strict_mode,
            flavor,
            reject_root_union: bool_value(value, "rejectRootObjectUnion", false)?,
            retry_on_grammar_error: bool_value(value, "retryWithoutStrictOnGrammarError", false)?,
        })
    }
}

pub(crate) fn native_chat_initial_policy(
    model: &Model,
    request: Option<&ReasoningRequest>,
    policy: Option<&OpenAIReasoningPolicy>,
    base: &OpenAICompat,
    requested_choice: Option<&Value>,
) -> Result<NativeChatInitialPolicy, ProviderError> {
    let Some(policy) = policy_required(request, policy)? else {
        return Ok(NativeChatInitialPolicy {
            compat: base.clone(),
            supports_tool_choice: true,
            supports_forced_tool_choice: true,
            supports_named_tool_choice: true,
            drop_auto_for_reasoning: false,
            schema: NativeChatSchemaPolicy::default(),
        });
    };
    let resolved = resolve(policy, request, requested_choice)?;
    Ok(NativeChatInitialPolicy {
        compat: resolved.compat.messages.apply(base),
        supports_tool_choice: resolved.compat.tools,
        supports_forced_tool_choice: resolved.compat.forced,
        supports_named_tool_choice: resolved.compat.named,
        // openai-shared.ts:878–890 uses the explicit requested reasoning,
        // rather than inventing a request from a model's default effort.
        drop_auto_for_reasoning: resolved.compat.suppress_any
            && model.reasoning
            && request.is_some_and(|request| request.effort.is_some() && !request.disable_reasoning),
        schema: NativeChatSchemaPolicy {
            // Fixed grammar fallback reads original model.compat, while tool
            // adaptation uses the initial requested-choice effective compat.
            retry_on_grammar_error: policy.compat.chat_schema.retry_on_grammar_error,
            ..resolved.compat.chat_schema
        },
    })
}

pub(crate) fn apply_chat(
    body: &mut Value,
    model: &Model,
    request: Option<&ReasoningRequest>,
    policy: Option<&OpenAIReasoningPolicy>,
    base: &OpenAICompat,
) -> Result<OpenAICompat, ProviderError> {
    if let Some(policy) = policy_required(request, policy)? {
        prepare_tool_choice(body, policy, request)?;
    }
    apply_chat_final_policy(body, model, request, policy, base)
}

/// Encode only the final, already prepared wire choice. Native callers keep
/// tool filtering and getter/callback effects in their original graph.
pub(crate) fn apply_chat_final_policy(
    body: &mut Value,
    model: &Model,
    request: Option<&ReasoningRequest>,
    policy: Option<&OpenAIReasoningPolicy>,
    base: &OpenAICompat,
) -> Result<OpenAICompat, ProviderError> {
    let Some(policy) = policy_required(request, policy)? else {
        return Ok(base.clone());
    };
    let resolved = resolve(policy, request, body.get("tool_choice"))?;
    let converter = resolved.compat.messages.apply(base);
    // Fixed Chat buildParams converts with finalPolicy.compat, then the
    // reasoning encoder resolves that effective compat again.
    let resolved = resolve_compat(policy, resolved.compat, None, Some(&resolved.request), body.get("tool_choice"))?;
    body["model"] = json!(policy.wire_model_id(model, resolved.request.effort));
    let compat = resolved.compat;
    if compat.qwen_preserve {
        if compat.dialect == "qwen" {
            body["preserve_thinking"] = json!(true);
        }
        merge_object(body, "chat_template_kwargs", json!({"preserve_thinking":true}));
    }
    if (!policy.supported && !resolved.disabled) || !compat.params {
        return Ok(converter);
    }
    if resolved.enabled {
        match compat.disable {
            DisableMode::Zai => {
                if resolved.wire.as_deref() == Some("none") {
                    encode_disabled_chat(body, compat.disable);
                    return Ok(converter);
                }
                body["thinking"] = json!({"type":"enabled"});
                if let Some(keep) = compat.keep.as_ref().filter(|v| *v == "all") {
                    body["thinking"]["keep"] = keep.clone();
                }
                if compat.dialect == "kimi" {
                    if let Some(wire) = &resolved.wire {
                        body["thinking"]["effort"] = json!(wire);
                    }
                } else if compat.effort
                    && let Some(wire) = &resolved.wire
                {
                    body["reasoning_effort"] = json!(wire);
                }
            }
            DisableMode::Qwen | DisableMode::QwenTemplate => {
                if compat.disable == DisableMode::Qwen {
                    body["enable_thinking"] = json!(true);
                } else {
                    merge_object(body, "chat_template_kwargs", json!({"enable_thinking":true}));
                }
                if compat.qwen_effort
                    && let Some(wire) = &resolved.wire
                {
                    if compat.disable == DisableMode::Qwen {
                        body["reasoning_effort"] = json!(wire);
                    }
                    merge_object(body, "chat_template_kwargs", json!({"reasoning_effort":wire}));
                }
            }
            DisableMode::ChatTemplate => {
                merge_object(body, "chat_template_kwargs", json!({"thinking":true}));
                if let Some(wire) = &resolved.wire {
                    merge_object(body, "chat_template_kwargs", json!({"reasoning_effort":wire}));
                }
            }
            DisableMode::OpenRouter => {
                if let Some(wire) = &resolved.wire {
                    body["reasoning"] = json!({"effort":wire});
                }
            }
            _ => {
                if !compat.omit
                    && compat.effort
                    && let Some(wire) = &resolved.wire
                {
                    body["reasoning_effort"] = json!(wire);
                }
            }
        }
        if model.provider == "cline-pass"
            && policy.thinking.as_ref().is_some_and(|t| t.budget)
            && let Some(effort) = resolved.request.effort
            && let Some(budget) = resolved
                .request
                .thinking_budgets
                .get(&effort)
                .or_else(|| policy.thinking.as_ref().and_then(|t| t.budgets.get(&effort)))
        {
            body.as_object_mut().unwrap().remove("reasoning_effort");
            merge_object(body, "reasoning", json!({"max_tokens":budget}));
        }
    } else if resolved.disabled {
        if resolved.caller
            && resolved.request.effort.is_none()
            && matches!(compat.disable, DisableMode::Lowest | DisableMode::None)
            && resolved.wire.is_some()
        {
            body["reasoning_effort"] = json!(resolved.wire);
        } else {
            encode_disabled_chat(body, compat.disable);
        }
        if resolved.forced_conflict && compat.disable == DisableMode::OpenRouter && compat.openrouter {
            body.as_object_mut().unwrap().remove("reasoning");
        }
    }
    Ok(converter)
}

fn encode_disabled_chat(body: &mut Value, mode: DisableMode) {
    body.as_object_mut().unwrap().remove("reasoning_effort");
    match mode {
        DisableMode::None => body["reasoning_effort"] = json!("none"),
        DisableMode::Zai => body["thinking"] = json!({"type":"disabled"}),
        DisableMode::Qwen => body["enable_thinking"] = json!(false),
        DisableMode::QwenTemplate => merge_object(body, "chat_template_kwargs", json!({"enable_thinking":false})),
        DisableMode::ChatTemplate => merge_object(body, "chat_template_kwargs", json!({"thinking":false})),
        DisableMode::OpenRouter | DisableMode::Cline => body["reasoning"] = json!({"enabled":false}),
        DisableMode::Venice => merge_object(body, "venice_parameters", json!({"disable_thinking":true})),
        _ => {
            body.as_object_mut().unwrap().remove("reasoning");
        }
    }
}

pub(crate) fn apply_responses(
    body: &mut Value,
    model: &Model,
    request: Option<&ReasoningRequest>,
    policy: Option<&OpenAIReasoningPolicy>,
) -> Result<(), ProviderError> {
    let Some(policy) = policy_required(request, policy)? else {
        return Ok(());
    };
    prepare_tool_choice(body, policy, request)?;
    let resolved = resolve(policy, request, body.get("tool_choice"))?;
    body["model"] = json!(policy.wire_model_id(model, resolved.request.effort));
    // Reasoning policy owns only its encrypted item. Preserve caller include.
    if let Some(include) = body.get_mut("include").and_then(Value::as_array_mut) {
        include.retain(|item| item != "reasoning.encrypted_content");
    }
    if body.get("include").and_then(Value::as_array).is_some_and(Vec::is_empty) {
        body.as_object_mut().unwrap().remove("include");
    }
    if resolved.request.force_reasoning_off {
        body["reasoning"] = json!({"effort":"none"});
        if resolved.compat.off_juice {
            let juice = match resolved.request.effort.unwrap_or(ReasoningEffort::Medium) {
                ReasoningEffort::Minimal => 2,
                ReasoningEffort::Low => 4,
                ReasoningEffort::Medium => 8,
                ReasoningEffort::High => 48,
                ReasoningEffort::XHigh => 112,
                ReasoningEffort::Max => 960,
            };
            body["input"].as_array_mut().unwrap().push(json!({"role":"developer","content":[{"type":"input_text","text":format!("# Juice: {juice} !important")}]}));
        }
        return Ok(());
    }
    if !policy.supported {
        responses_mode(body, policy);
        return Ok(());
    }
    if resolved.compat.encrypted {
        if !body.get("include").is_some_and(Value::is_array) {
            body["include"] = json!([]);
        }
        body["include"].as_array_mut().unwrap().push(json!("reasoning.encrypted_content"));
    }
    if resolved.disabled {
        if resolved.compat.disable == DisableMode::OpenRouter {
            body["reasoning"] = json!({"enabled":false});
        } else if matches!(resolved.compat.disable, DisableMode::Lowest | DisableMode::None)
            && resolved.wire.is_some()
            && !resolved.compat.omit
            && resolved.compat.effort
        {
            body["reasoning"] = json!({"effort":resolved.wire});
        }
        responses_mode(body, policy);
        return Ok(());
    }
    let summary = if policy.compat.summary {
        resolved.request.summary
    } else if resolved.request.effort.is_some() {
        ReasoningSummary::Hidden
    } else {
        ReasoningSummary::ProviderDefault
    };
    if resolved.request.effort.is_some() || summary != ReasoningSummary::ProviderDefault {
        if resolved.compat.omit || !resolved.compat.effort {
            if let Some(summary) = summary.wire() {
                body["reasoning"] = json!({"summary":summary});
            }
        } else {
            let effort = resolved.wire.unwrap_or_else(|| {
                policy.map_effort(resolved.compat, resolved.request.effort.unwrap_or(ReasoningEffort::Medium))
            });
            body["reasoning"] = json!({"effort":effort});
            if summary != ReasoningSummary::Hidden {
                body["reasoning"]["summary"] = json!(summary.wire().unwrap_or("auto"));
            }
        }
    }
    responses_mode(body, policy);
    Ok(())
}

fn responses_mode(body: &mut Value, policy: &OpenAIReasoningPolicy) {
    if let Some(mode) = &policy.mode {
        merge_object(body, "reasoning", json!({"mode":mode}));
    }
}

pub(crate) fn apply_codex(
    body: &mut Value,
    model: &Model,
    request: Option<&ReasoningRequest>,
    policy: Option<&OpenAIReasoningPolicy>,
) -> Result<(), ProviderError> {
    let Some(policy) = policy_required(request, policy)? else {
        return Ok(());
    };
    let request = policy.normalize(request)?;
    body["model"] = json!(policy.wire_model_id(model, request.effort));
    body.as_object_mut().unwrap().remove("reasoning");
    // SimpleStreamOptions.disableReasoning is not carried to the fixed Codex
    // transformer. Only forceReasoningOff becomes its reasoningOff switch.
    if request.force_reasoning_off {
        body["reasoning"] = json!({"effort":"none"});
    } else if let Some(effort) = request.effort {
        if !policy.supported_efforts().contains(&effort) {
            return Err(ProviderError::Config(format!(
                "Codex effort {} is not supported by this model policy",
                effort.as_str()
            )));
        }
        let mapped = policy.map_effort(&policy.compat, effort);
        if !["none", "minimal", "low", "medium", "high", "xhigh", "max"].contains(&mapped.as_str()) {
            return Err(ProviderError::Config(format!(
                "Effort map produced invalid Codex reasoning effort {mapped:?}"
            )));
        }
        body["reasoning"] = json!({"effort":mapped});
        if policy.compat.summary && request.summary != ReasoningSummary::Hidden {
            body["reasoning"]["summary"] = json!(request.summary.wire().unwrap_or("auto"));
        }
    }
    if !request.force_reasoning_off
        && let Some(mode) = &policy.mode
    {
        merge_object(body, "reasoning", json!({"mode":mode}));
    }
    Ok(())
}

fn merge_object(body: &mut Value, key: &str, fields: Value) {
    if !body.get(key).is_some_and(Value::is_object) {
        body[key] = Value::Object(Map::new());
    }
    body[key].as_object_mut().unwrap().extend(fields.as_object().unwrap().clone());
}
fn invalid(path: &str, value: &Value) -> ProviderError {
    ProviderError::Config(format!("Unsupported or invalid OpenAI thinking policy {path}: {value}"))
}
fn bool_option(value: &Value, key: &str) -> Result<Option<bool>, ProviderError> {
    value.get(key).filter(|v| !v.is_null()).map(|v| v.as_bool().ok_or_else(|| invalid(key, v))).transpose()
}
fn bool_value(value: &Value, key: &str, default: bool) -> Result<bool, ProviderError> {
    Ok(bool_option(value, key)?.unwrap_or(default))
}
fn string_option<'a>(value: &'a Value, key: &str) -> Result<Option<&'a str>, ProviderError> {
    value
        .get(key)
        .filter(|v| !v.is_null())
        .map(|v| v.as_str().filter(|s| !s.is_empty()).ok_or_else(|| invalid(key, v)))
        .transpose()
}
fn parse_effort(value: &Value, path: &str) -> Result<ReasoningEffort, ProviderError> {
    serde_json::from_value(value.clone()).map_err(|_| invalid(path, value))
}
fn effort_map(value: Option<&Value>, path: &str) -> Result<BTreeMap<ReasoningEffort, String>, ProviderError> {
    let mut result = BTreeMap::new();
    if let Some(value) = value.filter(|v| !v.is_null()) {
        for (key, wire) in value.as_object().ok_or_else(|| invalid(path, value))? {
            let effort = parse_effort(&json!(key), path)?;
            let wire =
                wire.as_str().filter(|s| !s.is_empty()).ok_or_else(|| invalid(&format!("{path}.{key}"), wire))?;
            result.insert(effort, wire.into());
        }
    }
    Ok(result)
}
fn budget_map(value: Option<&Value>, path: &str) -> Result<BTreeMap<ReasoningEffort, u64>, ProviderError> {
    let mut result = BTreeMap::new();
    if let Some(value) = value.filter(|v| !v.is_null()) {
        for (key, budget) in value.as_object().ok_or_else(|| invalid(path, value))? {
            result.insert(
                parse_effort(&json!(key), path)?,
                budget.as_u64().ok_or_else(|| invalid(&format!("{path}.{key}"), budget))?,
            );
        }
    }
    Ok(result)
}
