//! Fixed Google, Gemini CLI and Antigravity discovery and wire headers.

use super::*;
use crate::{
    catalog_rules::{compare_revision, parse_revision},
    model_collapse::VariantCollapseTable,
    model_wire_policy::classify_wire,
};
use futures::{
    FutureExt,
    future::{BoxFuture, Shared},
};
use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

pub const GOOGLE_GENERATIVE_AI_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";
pub const ANTIGRAVITY_PRIMARY_ENDPOINT: &str = "https://daily-cloudcode-pa.googleapis.com";
pub const ANTIGRAVITY_SANDBOX_ENDPOINT: &str = "https://daily-cloudcode-pa.sandbox.googleapis.com";
pub const DEFAULT_ANTIGRAVITY_VERSION: &str = "2.8.0";
pub const ANTIGRAVITY_VERSION_MANIFEST_URL: &str =
    "https://antigravity-hub-auto-updater-974169037036.us-central1.run.app/manifest/latest-arm64-mac.yml";

pub trait GoogleHeaderEnvironment: Send + Sync {
    fn get(&self, name: &str) -> Option<WireString>;
    fn platform(&self) -> WireString;
    fn arch(&self) -> WireString;
}
pub struct ProcessGoogleHeaderEnvironment;
impl GoogleHeaderEnvironment for ProcessGoogleHeaderEnvironment {
    fn get(&self, name: &str) -> Option<WireString> {
        let ara_name = name.replacen("PI_AI_", "ARA_AI_", 1);
        let value = std::env::var_os(&ara_name).or_else(|| std::env::var_os(name))?;
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            Some(WireString::from_units(value.encode_wide().collect()))
        }
        #[cfg(not(windows))]
        {
            Some(value.to_string_lossy().into_owned().into())
        }
    }
    fn platform(&self) -> WireString {
        match std::env::consts::OS {
            "windows" => "win32",
            "macos" => "darwin",
            other => other,
        }
        .into()
    }
    fn arch(&self) -> WireString {
        match std::env::consts::ARCH {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            "x86" => "ia32",
            other => other,
        }
        .into()
    }
}
#[derive(Default)]
struct VersionState {
    discovered: Option<WireString>,
    loading: Option<Shared<BoxFuture<'static, ()>>>,
}
pub struct GoogleHeaderRuntime {
    pub environment: Arc<dyn GoogleHeaderEnvironment>,
    state: Mutex<VersionState>,
}
impl GoogleHeaderRuntime {
    pub fn new(environment: Arc<dyn GoogleHeaderEnvironment>) -> Self {
        Self { environment, state: Mutex::new(VersionState::default()) }
    }
    pub fn for_host() -> Self {
        Self::new(Arc::new(ProcessGoogleHeaderEnvironment))
    }
    fn env_or(&self, key: &str, fallback: &str) -> WireString {
        self.environment.get(key).filter(|value| !value.is_empty()).unwrap_or_else(|| fallback.into())
    }
    pub fn get_gemini_cli_user_agent(&self, model_id: Option<&WireString>) -> WireString {
        join_wire(&[
            "GeminiCLI/".into(),
            self.env_or("PI_AI_GEMINI_CLI_VERSION", "0.46.0"),
            "/".into(),
            model_id.cloned().unwrap_or_else(|| "gemini-3.1-pro-preview".into()),
            " (".into(),
            self.environment.platform(),
            "; ".into(),
            self.environment.arch(),
            "; terminal)".into(),
        ])
    }
    pub fn get_gemini_cli_headers(&self, model_id: Option<&WireString>) -> Vec<(WireString, WireString)> {
        vec![
            ("User-Agent".into(), self.get_gemini_cli_user_agent(model_id)),
            (
                "Client-Metadata".into(),
                "ideType=IDE_UNSPECIFIED,platform=PLATFORM_UNSPECIFIED,pluginType=GEMINI".into(),
            ),
        ]
    }
    pub fn get_antigravity_version(&self) -> WireString {
        self.environment
            .get("PI_AI_ANTIGRAVITY_VERSION")
            .filter(|value| !value.is_empty())
            .or_else(|| self.state.lock().expect("version state poisoned").discovered.clone())
            .unwrap_or_else(|| DEFAULT_ANTIGRAVITY_VERSION.into())
    }
    pub fn get_antigravity_user_agent(&self) -> WireString {
        join_wire(&[
            "antigravity/hub/".into(),
            self.get_antigravity_version(),
            " (aidev_client; os_type=".into(),
            self.env_or("PI_AI_ANTIGRAVITY_OS", "darwin"),
            "; arch=".into(),
            self.env_or("PI_AI_ANTIGRAVITY_ARCH", "arm64"),
            "; cl=".into(),
            self.env_or("PI_AI_ANTIGRAVITY_CL", "963137146"),
            ")".into(),
        ])
    }
    pub async fn ensure_antigravity_version(
        self: &Arc<Self>,
        context: &CatalogContext,
        signal: Option<DiscoverySignal>,
    ) {
        let future = {
            let mut state = self.state.lock().expect("version state poisoned");
            if self.environment.get("PI_AI_ANTIGRAVITY_VERSION").is_some_and(|value| !value.is_empty())
                || state.discovered.is_some()
            {
                return;
            }
            if let Some(future) = &state.loading {
                future.clone()
            } else {
                let owner = Arc::clone(self);
                let transport = Arc::clone(context.version_transport.as_ref().unwrap_or(&context.transport));
                let task = tokio::spawn(async move {
                    let timeout_signal = DiscoverySignal::default();
                    let timeout = timeout_signal.clone();
                    // Native AbortSignal.timeout remains observable to a fetch
                    // override retaining its signal after successful return.
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(5000)).await;
                        timeout
                            .abort(DiscoveryError::named("TimeoutError", "The operation was aborted due to timeout"));
                    });
                    let combined =
                        signal.map_or(timeout_signal.clone(), |signal| DiscoverySignal::any(&[signal, timeout_signal]));
                    let reply = transport
                        .fetch(DiscoveryRequest {
                            url: ANTIGRAVITY_VERSION_MANIFEST_URL.into(),
                            headers: vec![
                                ("Cache-Control".into(), "no-cache".into()),
                                ("User-Agent".into(), "electron-builder".into()),
                            ],
                            signal: Some(combined),
                            ..Default::default()
                        })
                        .await;
                    let version = reply
                        .ok()
                        .filter(DiscoveryReply::ok)
                        .and_then(|reply| parse_antigravity_manifest_version(&reply.text()));
                    let mut state = owner.state.lock().expect("version state poisoned");
                    state.discovered = version;
                    if state.discovered.is_none() {
                        state.loading = None;
                    }
                });
                let future = task.map(|_| ()).boxed().shared();
                state.loading = Some(future.clone());
                future
            }
        };
        future.await;
    }
}
fn join_wire(parts: &[WireString]) -> WireString {
    WireString::from_units(parts.iter().flat_map(|part| part.units().iter().copied()).collect())
}

pub fn parse_antigravity_manifest_version(text: &WireString) -> Option<WireString> {
    let mut expression = crate::js_regex::JsRegExp::new(
        WireString::from(r#"^\s*version\s*:\s*(?:"([^"]*)"|'([^']*)'|([^\s#]+))\s*(?:#.*)?$"#),
        "",
    )
    .ok()?;
    let mut valid = crate::js_regex::JsRegExp::new(r"^\d+\.\d+\.\d+$".into(), "").ok()?;
    for line in text.units().split(|&unit| unit == u16::from(b'\n')) {
        let line = if line.last() == Some(&u16::from(b'\r')) { &line[..line.len() - 1] } else { line };
        if let Some(found) = expression.exec(&WireString::from_units(line.to_vec())) {
            let value = found.captures.iter().skip(1).flatten().next().cloned().unwrap_or_else(|| "".into());
            let version = js_trim(&value);
            return valid.exec(&version).map(|_| version);
        }
    }
    None
}

#[derive(Debug, Clone)]
pub struct GeminiDiscoveryOptions {
    pub api_key: WireString,
    pub base_url: Option<WireString>,
    pub page_size: Option<f64>,
    pub max_pages: Option<f64>,
    pub signal: Option<DiscoverySignal>,
}
impl GeminiDiscoveryOptions {
    pub fn new(api_key: impl Into<WireString>) -> Self {
        Self { api_key: api_key.into(), base_url: None, page_size: None, max_pages: None, signal: None }
    }
}
fn positive_int(value: Option<f64>, fallback: Option<f64>) -> Option<f64> {
    value.filter(|value| value.is_finite() && *value > 0.0).map(f64::floor).filter(|value| *value > 0.0).or(fallback)
}
fn number_or_null(value: Option<f64>) -> WireValue {
    value.map_or(WireValue::Null, WireValue::Number)
}
fn positive_number(value: Option<&WireValue>, fallback: f64) -> f64 {
    value.and_then(WireValue::as_number).filter(|value| value.is_finite() && *value > 0.0).unwrap_or(fallback)
}

pub async fn fetch_gemini_models(context: &CatalogContext, options: &GeminiDiscoveryOptions) -> DiscoveryResult {
    if js_trim(&options.api_key).is_empty() {
        return Ok(None);
    }
    let base = js_trim(options.base_url.as_ref().unwrap_or(&GOOGLE_GENERATIVE_AI_BASE_URL.into()));
    let base = if base.is_empty() { GOOGLE_GENERATIVE_AI_BASE_URL.into() } else { trim_trailing_slashes(&base) };
    let page_size = positive_int(options.page_size, Some(100.0)).expect("default page size");
    let max_pages = positive_int(options.max_pages, Some(25.0)).expect("default page limit");
    let references = bundled_references(&"google".into());
    let mut models = Vec::new();
    let mut positions = HashMap::new();
    let mut seen_tokens = HashSet::new();
    let mut next_token: Option<WireString> = None;
    let mut page = 0.0;
    while page < max_pages {
        let mut url = base.clone();
        url.append_str("/models");
        // URL construction occurs outside the source fetch catch and may throw.
        let mut url = reqwest::Url::parse(&String::from_utf16_lossy(url.units()))
            .map_err(|_| DiscoveryError::named("TypeError", "Invalid URL"))?;
        {
            let mut pairs: Vec<(String, String)> =
                url.query_pairs().map(|(key, value)| (key.into_owned(), value.into_owned())).collect();
            let mut set = |key: &str, value: String| {
                let mut found = false;
                pairs.retain_mut(|(held, old)| {
                    if held != key {
                        return true;
                    }
                    if found {
                        return false;
                    }
                    *old = value.clone();
                    found = true;
                    true
                });
                if !found {
                    pairs.push((key.to_owned(), value));
                }
            };
            set("key", String::from_utf16_lossy(options.api_key.units()));
            set("pageSize", WireValue::Number(page_size).stringify());
            if let Some(token) = &next_token {
                set("pageToken", String::from_utf16_lossy(token.units()));
            }
            url.query_pairs_mut().clear().extend_pairs(pairs);
        }
        let reply = context
            .transport
            .fetch(DiscoveryRequest { url: url.as_str().into(), signal: options.signal.clone(), ..Default::default() })
            .await;
        let Some(payload) = reply.ok().filter(DiscoveryReply::ok).and_then(|reply| reply.json().ok()) else {
            return Ok(None);
        };
        if !matches!(payload.value, WireValue::Object(_)) {
            return Ok(None);
        }
        let items = match payload.get("models") {
            None => &[][..],
            Some(WireValue::Array(items)) => items.as_slice(),
            _ => return Ok(None),
        };
        for item in items {
            if !matches!(item, WireValue::Object(_)) {
                continue;
            }
            if item.get("supportedGenerationMethods").is_some_and(|methods|!matches!(methods,WireValue::Array(values) if values.iter().all(|value|matches!(value,WireValue::String(_))))){continue}
            let Some(name) = item.get("name").and_then(WireValue::as_string).filter(|name| !name.is_empty()) else {
                continue;
            };
            let mut id = js_trim(name);
            if id.is_empty() {
                continue;
            }
            if id.units().starts_with(&"models/".encode_utf16().collect::<Vec<_>>()) {
                id = WireString::from_units(id.units()[7..].to_vec());
            }
            if id.is_empty() {
                continue;
            }
            if !item.get("supportedGenerationMethods").and_then(WireValue::as_array).is_some_and(|methods| {
                methods
                    .iter()
                    .any(|method| method.as_string().is_some_and(|method| method.equals_ascii("generateContent")))
            }) {
                continue;
            }
            let reference = references.get(&id);
            let context_window = positive_int(
                item.get("inputTokenLimit").and_then(WireValue::as_number),
                reference.and_then(|model| model.get("contextWindow")).and_then(WireValue::as_number),
            );
            let max_tokens = positive_int(
                item.get("outputTokenLimit").and_then(WireValue::as_number),
                reference.and_then(|model| model.get("maxTokens")).and_then(WireValue::as_number),
            );
            let display = item
                .get("displayName")
                .and_then(WireValue::as_string)
                .map(js_trim)
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| reference.map(|model| model_string(model, "name")).unwrap_or_else(|| id.clone()));
            let mut model = if let Some(reference) = reference {
                (**reference).clone()
            } else {
                let lower = js_lower(&id);
                let reasoning =
                    wire_contains(&lower, "thinking") || wire_contains(&lower, "pro") || wire_contains(&lower, "2.5");
                let images = wire_contains(&lower, "vision")
                    || wire_contains(&lower, "image")
                    || wire_contains(&lower, "gemini");
                VariantSpec::from_wire(wire_object(&[
                    ("id", wire_string(id.clone())),
                    ("name", wire_string(display.clone())),
                    ("api", wire_string("google-generative-ai")),
                    ("provider", wire_string("google")),
                    ("baseUrl", wire_string(base.clone())),
                    ("reasoning", WireValue::Bool(reasoning)),
                    (
                        "input",
                        WireValue::Array(if images {
                            vec![wire_string("text"), wire_string("image")]
                        } else {
                            vec![wire_string("text")]
                        }),
                    ),
                    ("cost", zero_cost()),
                    ("contextWindow", number_or_null(context_window)),
                    ("maxTokens", number_or_null(max_tokens)),
                ]))
            };
            for (key, value) in [
                ("id", wire_string(id.clone())),
                ("name", wire_string(display)),
                ("baseUrl", wire_string(base.clone())),
                ("contextWindow", number_or_null(context_window)),
                ("maxTokens", number_or_null(max_tokens)),
            ] {
                model.set(key, value);
            }
            let model = Arc::new(model);
            if let Some(&index) = positions.get(&id) {
                models[index] = model
            } else {
                positions.insert(id, models.len());
                models.push(model)
            }
        }
        let token =
            payload.get("nextPageToken").and_then(WireValue::as_string).map(js_trim).filter(|token| !token.is_empty());
        let Some(token) = token else { break };
        if !seen_tokens.insert(token.clone()) {
            break;
        }
        next_token = Some(token);
        page += 1.0;
    }
    context.sort_by_id(&mut models);
    Ok(Some(models))
}

#[derive(Clone)]
pub struct AntigravityDiscoveryOptions {
    pub token: WireString,
    pub endpoint: Option<WireString>,
    pub project: Option<WireString>,
    pub user_agent: Option<WireString>,
    pub signal: Option<DiscoverySignal>,
    pub collapse_table: Option<Arc<VariantCollapseTable>>,
}
impl AntigravityDiscoveryOptions {
    pub fn new(token: impl Into<WireString>) -> Self {
        Self {
            token: token.into(),
            endpoint: None,
            project: None,
            user_agent: None,
            signal: None,
            collapse_table: None,
        }
    }
}
pub async fn fetch_antigravity_discovery_models(
    context: &CatalogContext,
    options: &AntigravityDiscoveryOptions,
) -> DiscoveryResult {
    if options.user_agent.is_none() {
        context.runtime.google_headers.ensure_antigravity_version(context, options.signal.clone()).await;
    }
    let endpoints = if let Some(endpoint) = options.endpoint.as_ref().filter(|endpoint| !endpoint.is_empty()) {
        vec![trim_trailing_slashes(endpoint)]
    } else {
        vec![ANTIGRAVITY_PRIMARY_ENDPOINT.into(), ANTIGRAVITY_SANDBOX_ENDPOINT.into()]
    };
    for endpoint in endpoints {
        let mut url = endpoint.clone();
        url.append_str("/v1internal:fetchAvailableModels");
        let reply = context
            .transport
            .fetch(DiscoveryRequest {
                url,
                method: HttpMethod::Post,
                headers: vec![
                    ("Authorization".into(), join_wire(&["Bearer ".into(), options.token.clone()])),
                    ("Content-Type".into(), "application/json".into()),
                    (
                        "User-Agent".into(),
                        options
                            .user_agent
                            .clone()
                            .unwrap_or_else(|| context.runtime.google_headers.get_antigravity_user_agent()),
                    ),
                ],
                body: Some(b"{}".to_vec()),
                signal: options.signal.clone(),
                ..Default::default()
            })
            .await;
        let Some(payload) = reply.ok().filter(DiscoveryReply::ok).and_then(|reply| reply.json().ok()) else { continue };
        if !matches!(payload.value, WireValue::Object(_)) {
            continue;
        }
        let entries: Vec<(WireString, WireValue)> = match payload.get("models") {
            Some(WireValue::Object(entries)) => entries.clone(),
            Some(WireValue::Array(values)) => {
                values.iter().enumerate().map(|(index, value)| (index.to_string().into(), value.clone())).collect()
            }
            _ => Vec::new(),
        };
        let mut models = Vec::new();
        for (id, model) in entries {
            // The original normalized {} assigns through Object.prototype's
            // __proto__ setter, so this key never becomes an own model entry.
            if id.equals_ascii("__proto__") {
                continue;
            }
            if !matches!(model, WireValue::Object(_)) {
                continue;
            }
            if ["chat_20706", "chat_23310", "gemini-2.5-pro"].iter().any(|deny| id.equals_ascii(deny))
                || matches!(model.get("isInternal"), Some(WireValue::Bool(true)))
            {
                continue;
            }
            let name = model
                .get("displayName")
                .and_then(WireValue::as_string)
                .filter(|name| !name.is_empty())
                .cloned()
                .unwrap_or_else(|| id.clone());
            let images = matches!(model.get("supportsImages"), Some(WireValue::Bool(true)));
            let reasoning = matches!(model.get("supportsThinking"), Some(WireValue::Bool(true)));
            models.push(Arc::new(VariantSpec::from_wire(wire_object(&[
                ("id", wire_string(id)),
                ("name", wire_string(name)),
                ("api", wire_string("google-gemini-cli")),
                ("provider", wire_string("google-antigravity")),
                ("baseUrl", wire_string(endpoint.clone())),
                ("reasoning", WireValue::Bool(reasoning)),
                (
                    "input",
                    WireValue::Array(if images {
                        vec![wire_string("text"), wire_string("image")]
                    } else {
                        vec![wire_string("text")]
                    }),
                ),
                ("cost", zero_cost()),
                ("contextWindow", WireValue::Number(positive_number(model.get("maxTokens"), 200_000.0))),
                ("maxTokens", WireValue::Number(positive_number(model.get("maxOutputTokens"), 64_000.0))),
            ]))));
        }
        let mut models = context
            .runtime
            .collapse
            .lock()
            .expect("collapse runtime poisoned")
            .collapse_variants(&models, options.collapse_table.as_ref())?;
        context.sort_by_name(&mut models);
        return Ok(Some(models));
    }
    Ok(None)
}

#[derive(Clone)]
pub struct GeminiCliQuotaOptions {
    pub token: WireString,
    pub endpoint: Option<WireString>,
    pub project_id: Option<WireString>,
    pub signal: Option<DiscoverySignal>,
    pub collapse_table: Option<Arc<VariantCollapseTable>>,
}
impl GeminiCliQuotaOptions {
    pub fn new(token: impl Into<WireString>) -> Self {
        Self { token: token.into(), endpoint: None, project_id: None, signal: None, collapse_table: None }
    }
}
pub async fn fetch_gemini_cli_quota_models(
    context: &CatalogContext,
    options: &GeminiCliQuotaOptions,
) -> DiscoveryResult {
    let endpoint = options
        .endpoint
        .as_ref()
        .map(js_trim)
        .filter(|endpoint| !endpoint.is_empty())
        .unwrap_or_else(|| "https://cloudcode-pa.googleapis.com".into());
    let endpoint = trim_trailing_slashes(&endpoint);
    let mut headers = vec![
        ("Authorization".into(), join_wire(&["Bearer ".into(), options.token.clone()])),
        ("Content-Type".into(), "application/json".into()),
    ];
    headers.extend(context.runtime.google_headers.get_gemini_cli_headers(None));
    let project = if let Some(project) = &options.project_id {
        Some(project.clone())
    } else {
        let mut url = endpoint.clone();
        url.append_str("/v1internal:loadCodeAssist");
        let body =
            br#"{"metadata":{"ideType":"IDE_UNSPECIFIED","platform":"PLATFORM_UNSPECIFIED","pluginType":"GEMINI"}}"#
                .to_vec();
        context
            .transport
            .fetch(DiscoveryRequest {
                url,
                method: HttpMethod::Post,
                headers: headers.clone(),
                body: Some(body),
                signal: options.signal.clone(),
                ..Default::default()
            })
            .await
            .ok()
            .filter(DiscoveryReply::ok)
            .and_then(|reply| reply.json().ok())
            .filter(|payload| matches!(payload.value, WireValue::Object(_)))
            .and_then(|payload| match payload.get("cloudaicompanionProject") {
                Some(WireValue::String(value)) => Some(value.clone()),
                Some(WireValue::Object(_)) => payload
                    .get("cloudaicompanionProject")
                    .and_then(|value| value.get("id"))
                    .and_then(WireValue::as_string)
                    .cloned(),
                _ => None,
            })
    };
    let mut url = endpoint.clone();
    url.append_str("/v1internal:retrieveUserQuota");
    let body = if let Some(project) = project.filter(|project| !project.is_empty()) {
        wire_object(&[("project", wire_string(project))]).stringify().into_bytes()
    } else {
        b"{}".to_vec()
    };
    let reply = context
        .transport
        .fetch(DiscoveryRequest {
            url,
            method: HttpMethod::Post,
            headers,
            body: Some(body),
            signal: options.signal.clone(),
            ..Default::default()
        })
        .await;
    let Some(payload) = reply.ok().filter(DiscoveryReply::ok).and_then(|reply| reply.json().ok()) else {
        return Ok(None);
    };
    if !matches!(payload.value, WireValue::Object(_)) {
        return Ok(None);
    }
    let references = bundled_references(&"google-gemini-cli".into());
    let mut seen = HashSet::new();
    let mut models = Vec::new();
    for bucket in payload.get("buckets").and_then(WireValue::as_array).unwrap_or(&[]) {
        if !matches!(bucket, WireValue::Object(_)) {
            continue;
        }
        let Some(id) = bucket.get("modelId").and_then(WireValue::as_string).map(js_trim).filter(|id| !id.is_empty())
        else {
            continue;
        };
        if seen.contains(&id) {
            continue;
        }
        let identity = classify_wire(&"google-gemini-cli".into(), &id, true)?;
        if !identity.get("class").and_then(WireValue::as_string).is_some_and(|class| class.equals_ascii("gemini")) {
            continue;
        }
        seen.insert(id.clone());
        let model = if let Some(reference) = references.get(&id) {
            let mut model = (**reference).clone();
            model.set("baseUrl", wire_string(endpoint.clone()));
            model
        } else {
            let revision = identity
                .get("revision")
                .and_then(WireValue::as_string)
                .and_then(|revision| revision.to_utf8().ok())
                .and_then(|revision| parse_revision(&revision));
            let reasoning = revision.is_some_and(|revision| compare_revision(revision, [2, 5, 0]) >= 0);
            VariantSpec::from_wire(wire_object(&[
                ("id", wire_string(id.clone())),
                ("name", wire_string(id)),
                ("api", wire_string("google-gemini-cli")),
                ("provider", wire_string("google-gemini-cli")),
                ("baseUrl", wire_string(endpoint.clone())),
                ("reasoning", WireValue::Bool(reasoning)),
                ("input", WireValue::Array(vec![wire_string("text"), wire_string("image")])),
                ("cost", zero_cost()),
                ("contextWindow", WireValue::Number(1_048_576.0)),
                ("maxTokens", WireValue::Number(65_536.0)),
            ]))
        };
        models.push(Arc::new(model));
    }
    let mut models = context
        .runtime
        .collapse
        .lock()
        .expect("collapse runtime poisoned")
        .collapse_variants(&models, options.collapse_table.as_ref())?;
    context.sort_by_name(&mut models);
    Ok(Some(models))
}

#[derive(Debug, Clone, PartialEq)]
pub struct AntigravityModelWireProfile {
    pub model_enum: Option<&'static str>,
    pub max_output_tokens: u32,
}
pub fn get_antigravity_model_wire_profile(id: &WireString) -> Option<AntigravityModelWireProfile> {
    let (model_enum, max_output_tokens) = if id.equals_ascii("gemini-3.5-flash-extra-low") {
        (Some("MODEL_PLACEHOLDER_M187"), 65536)
    } else if id.equals_ascii("gemini-3.5-flash-low") {
        (Some("MODEL_PLACEHOLDER_M20"), 65536)
    } else if id.equals_ascii("gemini-3-flash-agent") {
        (Some("MODEL_PLACEHOLDER_M132"), 65536)
    } else if id.equals_ascii("gemini-3.1-pro-low") {
        (Some("MODEL_PLACEHOLDER_M36"), 65535)
    } else if id.equals_ascii("gemini-pro-agent") {
        (Some("MODEL_PLACEHOLDER_M16"), 65535)
    } else if id.equals_ascii("claude-sonnet-4-6") || id.equals_ascii("claude-opus-4-6-thinking") {
        (None, 64000)
    } else {
        return None;
    };
    Some(AntigravityModelWireProfile { model_enum, max_output_tokens })
}
