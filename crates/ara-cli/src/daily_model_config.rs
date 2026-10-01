//! Narrow daily CLI projection of the existing models configuration.
//!
//! The file/schema layer preserves the fixed OMP configuration surface. This
//! host projection accepts only options implemented by the selected adapter;
//! it does not perform discovery, execute key commands, or open credentials.
//! Explicit CLI values precede existing ARA environment values, then model
//! configuration, provider configuration, and protocol defaults.
//! This is an explicit CLI subset, not full OMP auth-storage precedence:
//! `auth:none` suppresses ambient credentials and Codex uses a stored account.
//! Descriptive name/cost/contextWindow metadata does not change execution or
//! the host's compaction threshold. Codex maxTokens remains capacity metadata.

use crate::model_config_file::{ModelConfigLoad, ModelsConfigFile};
use crate::model_route::{CredentialIdentity, ProtocolOptions, RequestAuthLease, is_credential_header};
use crate::models_config::ModelsConfig;
use ara_ai::Model;
use ara_ai::model_tokenizer::{ModelTokenizer, resolve_known_claude_tokenizer};
use ara_ai::providers::{openai_completions as chat, openai_responses as responses};
use serde_json::{Map, Value};
use std::{fmt, path::Path, time::Duration};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DailyApi {
    OpenAiCompletions,
    OpenAiResponses,
    OpenAiCodexResponses,
}

impl DailyApi {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAiCompletions => "openai-completions",
            Self::OpenAiResponses => "openai-responses",
            Self::OpenAiCodexResponses => "openai-codex-responses",
        }
    }

    fn parse(value: &str) -> Result<Self, DailyConfigError> {
        match value {
            "openai-completions" => Ok(Self::OpenAiCompletions),
            "openai-responses" => Ok(Self::OpenAiResponses),
            "openai-codex-responses" => Ok(Self::OpenAiCodexResponses),
            _ => Err(error("api", "is not supported by daily OpenAI configuration")),
        }
    }
}

/// Only explicit CLI values belong here. In particular clap's default API and
/// absent boolean flags must not be converted to Some values.
#[derive(Clone, Default)]
pub struct DailyOverrides {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub api: Option<String>,
    pub base_url: Option<String>,
    pub api_key_env: Option<String>,
    pub tokenizer: Option<String>,
    pub headers: Vec<(String, String)>,
    pub reasoning: Option<bool>,
    pub responses_stateful: Option<bool>,
    pub chat_replay_reasoning_content: Option<bool>,
    pub chat_mistral_compat: Option<bool>,
    pub max_tokens: Option<u64>,
    pub temperature: Option<f64>,
    /// Seconds, as in the existing CLI; zero disables both stream watchdogs.
    pub stream_idle_timeout: Option<f64>,
}

pub trait DailyEnvironment {
    fn get(&self, name: &str) -> Option<String>;
}

impl<F: Fn(&str) -> Option<String>> DailyEnvironment for F {
    fn get(&self, name: &str) -> Option<String> {
        self(name)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DailyGeneration {
    pub max_tokens: Option<u64>,
    pub temperature: Option<f64>,
}

/// Secret-bearing host input: intentionally neither Debug nor Serialize.
pub enum DailyAuthSource {
    Fixed(RequestAuthLease),
    OpenAiCodex,
}

/// Protocol settings contain no API key or credential header. Root binds the
/// private auth source and applies generation settings to all logical calls.
pub struct DailySelection {
    pub model: Model,
    pub api: DailyApi,
    pub protocol: ProtocolOptions,
    pub generation: DailyGeneration,
    pub auth_source: DailyAuthSource,
    pub loop_guard_policy: ara_ai::thinking_loop::LoopGuardPolicy,
}

/// Diagnostics never include configuration values (keys or header payloads).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DailyConfigError {
    pub field: String,
    pub message: String,
}

impl fmt::Display for DailyConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}
impl std::error::Error for DailyConfigError {}

fn error(field: &str, message: &str) -> DailyConfigError {
    DailyConfigError { field: field.into(), message: message.into() }
}

/// The host supplies `$ARA_HOME/agent/models.yml` or an explicit override.
/// A missing default file preserves the old flag/env route. A malformed file
/// must not silently become an empty configuration.
pub fn load_daily_config(path: &Path, explicit: bool) -> Result<Option<ModelsConfig>, DailyConfigError> {
    let mut file = ModelsConfigFile::new(path).map_err(|_| error("models-config", "unsupported file extension"))?;
    let result = file.try_load();
    match result {
        ModelConfigLoad::Ok(config) => Ok(Some(config)),
        ModelConfigLoad::Error(cause) => Err(error(
            "models-config",
            &format!("load failed at {} stage; correct the configuration file", cause.stage),
        )),
        ModelConfigLoad::NotFound if !file.take_warnings().is_empty() => {
            Err(error("models-config", "configuration migration failed; correct the source file or destination"))
        }
        ModelConfigLoad::NotFound if explicit => Err(error("models-config", "explicit file was not found")),
        ModelConfigLoad::NotFound => Ok(None),
    }
}

fn env_first(env: &dyn DailyEnvironment, names: &[&str]) -> Option<(String, String)> {
    names.iter().find_map(|name| env.get(name).filter(|v| !v.is_empty()).map(|v| ((*name).into(), v)))
}

fn text(map: &Map<String, Value>, key: &str) -> Option<String> {
    map.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn reject_fields(map: &Map<String, Value>, allowed: &[&str], path: &str) -> Result<(), DailyConfigError> {
    if let Some(key) = map.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(error(&format!("{path}/{key}"), "is not supported by daily model selection"));
    }
    Ok(())
}

fn object(value: Option<&Value>, field: &str) -> Result<Map<String, Value>, DailyConfigError> {
    match value {
        None => Ok(Map::new()),
        Some(Value::Object(value)) => Ok(value.clone()),
        _ => Err(error(field, "must be an object for daily model selection")),
    }
}

fn positive_cap(value: Option<&Value>) -> Result<Option<u64>, DailyConfigError> {
    value
        .map(|value| {
            let n = value.as_f64().ok_or_else(|| error("model/maxTokens", "must be a positive integer"))?;
            // serde_json numbers from the fixed configuration parser use JS f64.
            if n.is_finite() && n >= 1.0 && n.fract() == 0.0 && n < u64::MAX as f64 {
                Ok(n as u64)
            } else {
                Err(error("model/maxTokens", "must be a representable positive integer"))
            }
        })
        .transpose()
}

fn timeout(seconds: f64, field: &str) -> Result<Option<Duration>, DailyConfigError> {
    if !seconds.is_finite() || seconds < 0.0 {
        return Err(error(field, "must be a finite non-negative duration"));
    }
    if seconds == 0.0 {
        return Ok(None);
    }
    Duration::try_from_secs_f64(seconds).map(Some).map_err(|_| error(field, "duration is out of range"))
}

fn merge_header(headers: &mut Vec<(String, String)>, name: String, value: String) -> Result<(), DailyConfigError> {
    let parsed =
        http::header::HeaderName::from_bytes(name.as_bytes()).map_err(|_| error("headers", "invalid header name"))?;
    http::header::HeaderValue::from_str(&value).map_err(|_| error("headers", "invalid header value"))?;
    let name = parsed.as_str().to_owned();
    headers.retain(|(old, _)| !old.eq_ignore_ascii_case(&name));
    headers.push((name, value));
    Ok(())
}

fn config_headers(value: Option<&Value>, headers: &mut Vec<(String, String)>) -> Result<(), DailyConfigError> {
    for (name, value) in object(value, "headers")? {
        let value = value.as_str().ok_or_else(|| error("headers", "header value must be a string"))?;
        merge_header(headers, name, value.into())?;
    }
    Ok(())
}

const CHAT_COMPAT: &[&str] = &[
    "supportsDeveloperRole",
    "supportsUsageInStreaming",
    "maxTokensField",
    "requiresMistralToolIds",
    "requiresToolResultName",
    "requiresAssistantAfterToolResult",
    "requiresThinkingAsText",
    "streamIdleTimeoutMs",
];

/// Resolve an explicitly selected provider/model. Model IDs containing `/`
/// remain exact wire IDs; they are not guessed to be provider aliases.
pub fn resolve_daily_selection(
    config: Option<&ModelsConfig>,
    cli: &DailyOverrides,
    env: &dyn DailyEnvironment,
) -> Result<DailySelection, DailyConfigError> {
    let model_id = cli
        .model
        .clone()
        .or_else(|| env_first(env, &["ARA_MODEL", "ARA_TEST_MODEL_ID"]).map(|(_, v)| v))
        .filter(|id| !id.is_empty())
        .ok_or_else(|| error("model", "select an explicit model with --model or ARA_MODEL"))?;
    let provider_id = cli.provider.clone().or_else(|| env_first(env, &["ARA_PROVIDER"]).map(|(_, v)| v));
    let providers = config.and_then(|c| c.value().get("providers")).and_then(Value::as_object);
    let provider = match (&provider_id, providers) {
        (Some(id), Some(providers)) if id == "openai-codex" && !providers.contains_key(id) => Map::new(),
        (Some(id), Some(providers)) => object(
            Some(providers.get(id).ok_or_else(|| error("provider", "selected provider is not configured"))?),
            "provider",
        )?,
        (None, Some(providers)) if !providers.is_empty() => {
            return Err(error("provider", "select an exact configured provider with --provider"));
        }
        _ => Map::new(),
    };
    reject_fields(
        &provider,
        &["api", "baseUrl", "apiKey", "auth", "authHeader", "headers", "compat", "models", "modelOverrides"],
        "provider",
    )?;
    let model = match provider.get("models") {
        Some(Value::Array(models)) if !models.is_empty() => {
            let matches: Vec<_> =
                models.iter().filter(|m| m.get("id").and_then(Value::as_str) == Some(model_id.as_str())).collect();
            if matches.len() != 1 {
                return Err(error("model", "selected model must match exactly one configured model"));
            }
            object(Some(matches[0]), "model")?
        }
        _ => Map::new(),
    };
    if provider.get("modelOverrides").and_then(|v| v.get(&model_id)).is_some() {
        return Err(error(
            "provider/modelOverrides",
            "selected model overrides are not supported; use an explicit model entry",
        ));
    }
    reject_fields(
        &model,
        &[
            "id",
            "name",
            "api",
            "baseUrl",
            "reasoning",
            "input",
            "tokenizer",
            "maxTokens",
            "cost",
            "contextWindow",
            "supportsTools",
            "headers",
            "compat",
        ],
        "model",
    )?;
    if model.get("supportsTools").and_then(Value::as_bool) == Some(false) {
        return Err(error("model/supportsTools", "models disabling tools are not supported by this daily route"));
    }
    let api_text =
        cli.api.clone().or_else(|| text(&model, "api")).or_else(|| text(&provider, "api")).unwrap_or_else(|| {
            if provider_id.as_deref() == Some("openai-codex") { "openai-codex-responses" } else { "openai-completions" }
                .into()
        });
    let api = DailyApi::parse(&api_text)?;
    let base_url = cli
        .base_url
        .clone()
        .or_else(|| env_first(env, &["ARA_BASE_URL", "ARA_TEST_BASE_URL", "OPENROUTER_BASE_URL"]).map(|(_, v)| v))
        .or_else(|| text(&model, "baseUrl"))
        .or_else(|| text(&provider, "baseUrl"))
        .or_else(|| (api == DailyApi::OpenAiCodexResponses).then(|| "https://chatgpt.com/backend-api".into()))
        .ok_or_else(|| error("baseUrl", "set a configured baseUrl, --base-url or ARA_BASE_URL"))?;
    let url = reqwest::Url::parse(&base_url).map_err(|_| error("baseUrl", "must be an absolute HTTP(S) URL"))?;
    if !matches!(url.scheme(), "http" | "https") || !url.username().is_empty() || url.password().is_some() {
        return Err(error("baseUrl", "must be an HTTP(S) URL without embedded credentials"));
    }
    let openrouter = url.scheme() == "https"
        && url.host_str().is_some_and(|h| h == "openrouter.ai" || h.ends_with(".openrouter.ai"));
    let provider_id = provider_id.unwrap_or_else(|| if openrouter { "openrouter" } else { "openai-compatible" }.into());
    if (api == DailyApi::OpenAiCodexResponses) != (provider_id == "openai-codex") {
        return Err(error("provider/api", "openai-codex must use the openai-codex-responses API"));
    }
    let reasoning = cli.reasoning.or_else(|| model.get("reasoning").and_then(Value::as_bool)).unwrap_or(false);
    if reasoning && api == DailyApi::OpenAiCompletions {
        return Err(error("reasoning", "reasoning configuration is only supported by Responses routes"));
    }
    let configured_cap = positive_cap(model.get("maxTokens"))?;
    let generation = DailyGeneration {
        max_tokens: cli.max_tokens.or(if api == DailyApi::OpenAiCodexResponses { None } else { configured_cap }),
        temperature: cli.temperature,
    };
    if generation.max_tokens == Some(0) {
        return Err(error("max_tokens", "must be positive"));
    }
    if generation.temperature.is_some_and(|v| !v.is_finite()) {
        return Err(error("temperature", "must be finite"));
    }
    let tokenizer_name = cli
        .tokenizer
        .clone()
        .or_else(|| env_first(env, &["ARA_TOKENIZER"]).map(|(_, v)| v))
        .or_else(|| text(&model, "tokenizer"));
    let tokenizer = match tokenizer_name.as_deref() {
        None | Some("auto") => resolve_known_claude_tokenizer(&model_id),
        Some("none") => None,
        Some(name) => Some(
            ModelTokenizer::from_name(name)
                .ok_or_else(|| error("tokenizer", "tokenizer is not supported by this host"))?,
        ),
    };
    let mut headers = Vec::new();
    config_headers(provider.get("headers"), &mut headers)?;
    config_headers(model.get("headers"), &mut headers)?;
    for (name, value) in &cli.headers {
        merge_header(&mut headers, name.trim().into(), value.trim().into())?;
    }
    let mut compat = object(provider.get("compat"), "provider/compat")?;
    compat.extend(object(model.get("compat"), "model/compat")?);
    let authored_loop_guard = compat.remove("thinkingLoopGuard");
    if authored_loop_guard.as_ref().is_some_and(|value| !value.is_boolean()) {
        return Err(error("compat/thinkingLoopGuard", "must be a boolean"));
    }
    reject_fields(
        &compat,
        if api == DailyApi::OpenAiCompletions { CHAT_COMPAT } else { &["streamIdleTimeoutMs"] },
        "compat",
    )?;
    let watchdog = cli
        .stream_idle_timeout
        .or_else(|| compat.get("streamIdleTimeoutMs").and_then(Value::as_f64).map(|v| v / 1000.0));
    let watchdog = watchdog.map(|value| timeout(value, "stream_idle_timeout")).transpose()?;
    let stateful = cli.responses_stateful.unwrap_or(false);
    let replay = cli.chat_replay_reasoning_content.unwrap_or(false);
    let mistral = cli.chat_mistral_compat.unwrap_or(false);
    if stateful && api != DailyApi::OpenAiResponses {
        return Err(error("responses_stateful", "requires the ordinary openai-responses API"));
    }
    if (replay || mistral) && api != DailyApi::OpenAiCompletions {
        return Err(error("chat compatibility", "requires openai-completions"));
    }
    if replay && mistral {
        return Err(error("chat compatibility", "reasoning replay and Mistral compatibility are mutually exclusive"));
    }
    let supports_images =
        model.get("input").and_then(Value::as_array).map(|v| v.iter().any(|v| v.as_str() == Some("image")));
    let (credential_headers, ordinary_headers): (Vec<_>, Vec<_>) =
        headers.into_iter().partition(|(name, _)| is_credential_header(name));
    let auth_source =
        resolve_auth(&provider, cli, env, &provider_id, api, openrouter, credential_headers, &ordinary_headers)?;
    let protocol = match api {
        DailyApi::OpenAiCompletions => {
            let mut options = chat::StreamOptions {
                max_tokens: generation.max_tokens,
                temperature: generation.temperature,
                extra_headers: ordinary_headers,
                ..Default::default()
            };
            if let Some(v) = supports_images {
                options.compat.supports_images = v;
            }
            for (key, target) in [
                ("supportsDeveloperRole", &mut options.compat.supports_developer_role),
                ("supportsUsageInStreaming", &mut options.compat.supports_usage_in_streaming),
                ("requiresMistralToolIds", &mut options.compat.requires_mistral_tool_ids),
                ("requiresToolResultName", &mut options.compat.requires_tool_result_name),
                ("requiresAssistantAfterToolResult", &mut options.compat.requires_assistant_after_tool_result),
                ("requiresThinkingAsText", &mut options.compat.requires_thinking_as_text),
            ] {
                if let Some(v) = compat.get(key).and_then(Value::as_bool) {
                    *target = v;
                }
            }
            if let Some(value) = compat.get("maxTokensField").and_then(Value::as_str) {
                options.compat.max_tokens_field = if value == "max_completion_tokens" {
                    chat::MaxTokensField::MaxCompletionTokens
                } else {
                    chat::MaxTokensField::MaxTokens
                };
            }
            options.compat.requires_reasoning_content_on_all_assistant_turns = replay;
            if mistral {
                options.compat.requires_mistral_tool_ids = true;
                options.compat.requires_tool_result_name = true;
                options.compat.requires_assistant_after_tool_result = true;
                options.compat.requires_thinking_as_text = true;
            }
            if let Some(v) = watchdog {
                options.first_event_timeout = v;
                options.idle_timeout = v;
            }
            ProtocolOptions::Completions(options)
        }
        DailyApi::OpenAiResponses => {
            let mut options = responses::StreamOptions {
                extra_headers: ordinary_headers,
                stateful_responses: stateful,
                ..Default::default()
            };
            options.request.max_tokens = generation.max_tokens;
            options.request.temperature = generation.temperature;
            options.request.supports_images = supports_images.unwrap_or(false);
            if let Some(v) = watchdog {
                options.first_event_timeout = v;
                options.idle_timeout = v;
            }
            ProtocolOptions::Responses(options)
        }
        DailyApi::OpenAiCodexResponses => {
            if generation.max_tokens.is_some() || generation.temperature.is_some() {
                return Err(error("codex generation", "output caps and sampling are not supported by Codex"));
            }
            if supports_images == Some(true) {
                return Err(error(
                    "model/input",
                    "the daily Codex route currently supports text and function tools only",
                ));
            }
            if !codex_endpoint_allowed(&url, &base_url) {
                return Err(error("baseUrl", "Codex account authentication requires the fixed OpenAI endpoint"));
            }
            codex_protocol(ordinary_headers, watchdog)
        }
    };
    let execution_model = Model {
        id: model_id,
        api: api.as_str().into(),
        provider: provider_id,
        base_url,
        reasoning,
        max_tokens: configured_cap,
        tokenizer,
    };
    let mut loop_guard_policy = crate::model_route::resolved_loop_guard_policy(&execution_model)
        .map_err(|_| error("model", "cannot resolve the native loop guard policy"))?;
    if authored_loop_guard.is_some() {
        loop_guard_policy.semantic_heuristics = true;
    }
    Ok(DailySelection { model: execution_model, api, protocol, generation, auth_source, loop_guard_policy })
}

#[allow(clippy::too_many_arguments)]
fn resolve_auth(
    provider: &Map<String, Value>,
    cli: &DailyOverrides,
    env: &dyn DailyEnvironment,
    provider_id: &str,
    api: DailyApi,
    openrouter: bool,
    credential_headers: Vec<(String, String)>,
    ordinary_headers: &[(String, String)],
) -> Result<DailyAuthSource, DailyConfigError> {
    let auth = provider.get("auth").and_then(Value::as_str).unwrap_or("apiKey");
    if api == DailyApi::OpenAiCodexResponses {
        if cli.api_key_env.is_some()
            || provider.contains_key("apiKey")
            || !credential_headers.is_empty()
            || auth == "none"
            || provider.get("authHeader").and_then(Value::as_bool) == Some(false)
        {
            return Err(error("auth", "Codex requires its stored account credential without API-key overrides"));
        }
        if ordinary_headers.iter().any(|(name, _)| ara_ai::providers::openai_codex_responses::is_reserved_header(name))
        {
            return Err(error("headers", "Codex identity and session headers are host-owned"));
        }
        return Ok(DailyAuthSource::OpenAiCodex);
    }
    if auth == "oauth" {
        return Err(error("provider/auth", "OAuth is supported only for openai-codex"));
    }
    if provider.get("authHeader").and_then(Value::as_bool) == Some(false) {
        return Err(error("provider/authHeader", "disabling the native auth header is not supported"));
    }
    let (identity, key) = if let Some(name) = &cli.api_key_env {
        let value = env
            .get(name)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| error("api_key_env", "selected environment variable is missing or empty"))?;
        (CredentialIdentity::Environment { variable: name.clone() }, Some(value))
    } else if auth == "none" {
        if provider.contains_key("apiKey") || !credential_headers.is_empty() {
            return Err(error("provider/auth", "auth none conflicts with explicitly configured credentials"));
        }
        (CredentialIdentity::Keyless, None)
    } else if let Some((variable, key)) = env_first(env, &["ARA_API_KEY", "ARA_TEST_API_KEY"]) {
        (CredentialIdentity::Environment { variable }, Some(key))
    } else if let Some(value) = provider.get("apiKey").and_then(Value::as_str) {
        if value.starts_with('!') {
            return Err(error("provider/apiKey", "command values are not supported by the daily CLI"));
        }
        match env.get(value) {
            Some(key) if !key.is_empty() => (CredentialIdentity::Environment { variable: value.into() }, Some(key)),
            Some(_) => return Err(error("provider/apiKey", "referenced environment variable is empty")),
            None => (CredentialIdentity::Config { provider: provider_id.into() }, Some(value.into())),
        }
    } else {
        let names: &[&str] = if openrouter { &["OPENROUTER_API_KEY"] } else { &["ARA_API_KEY", "ARA_TEST_API_KEY"] };
        match env_first(env, names) {
            Some((variable, key)) => (CredentialIdentity::Environment { variable }, Some(key)),
            None => (CredentialIdentity::Keyless, None),
        }
    };
    Ok(DailyAuthSource::Fixed(RequestAuthLease::new(identity, key).with_headers(credential_headers)))
}

fn codex_protocol(headers: Vec<(String, String)>, watchdog: Option<Option<Duration>>) -> ProtocolOptions {
    let mut options =
        ara_ai::providers::openai_codex_responses::StreamOptions { extra_headers: headers, ..Default::default() };
    if let Some(value) = watchdog {
        options.first_event_timeout = value;
        options.idle_timeout = value;
    }
    ProtocolOptions::CodexResponses(options)
}

fn codex_endpoint_allowed(url: &reqwest::Url, raw: &str) -> bool {
    if raw.trim_end_matches('/') == "https://chatgpt.com/backend-api" {
        return true;
    }
    // Only an explicit fixture build can use a local fake account service.
    #[cfg(feature = "test-fixture")]
    {
        let loopback = url.host_str().is_some_and(|host| {
            host == "localhost"
                || host
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        });
        loopback && url.scheme() == "http" && url.query().is_none() && url.fragment().is_none()
    }
    #[cfg(not(feature = "test-fixture"))]
    {
        let _ = url;
        false
    }
}
