//! Fixed OMP provider-models/special.ts; Host-owned credentials and filesystem.
use super::{
    behavior,
    common::*,
    descriptor_types::{ModelManagerConfig, ProviderFactoryHost},
    static_data,
};
use crate::{
    bun_hash,
    catalog_discovery::{
        CatalogContext, DiscoveryError,
        codex::{CodexModelDiscoveryOptions, CodexModelDiscoveryResult, fetch_codex_models},
        cursor::{CursorModelDiscoveryOptions, fetch_cursor_usable_models},
        devin::{DEVIN_DEFAULT_BASE_URL, DevinModelDiscoveryOptions, fetch_devin_models},
        gitlab::{
            GitLabDiscoveryConfig, GitLabHost, NativeGitLabFs, build_gitlab_duo_workflow_fallback_model,
            fetch_gitlab_duo_workflow_models,
        },
    },
    model_collapse::{CollapseModelPolicy, SpecRef},
    model_identity_wire::{self as wire, text},
    model_manager::{ModelManagerOptions, RawModelValue},
    model_wire_policy::WireModelPolicy,
};
use ara_rpc::{WireString, WireValue};
use async_trait::async_trait;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};
pub static DEVIN_STATIC_MODELS: std::sync::LazyLock<Vec<SpecRef>> =
    std::sync::LazyLock::new(|| static_data::fixed_models("DEVIN_STATIC_MODELS"));
#[derive(Clone)]
pub struct OpenAiCodexAccount {
    pub access_token: WireString,
    pub account_id: Option<WireString>,
}
#[async_trait]
pub trait CodexAccountResolver: Send + Sync {
    async fn resolve_accounts(&self) -> Result<Option<Vec<OpenAiCodexAccount>>, DiscoveryError>;
}
#[derive(Clone, Default)]
pub struct OpenAiCodexModelManagerConfig {
    pub resolve_accounts: Option<Arc<dyn CodexAccountResolver>>,
    pub client_version: Option<WireString>,
    pub context: Option<CatalogContext>,
}
pub fn union_codex_models(
    results: &[(Option<WireString>, Option<CodexModelDiscoveryResult>)],
    host: &ProviderFactoryHost,
) -> Option<Vec<SpecRef>> {
    let mut models = Vec::new();
    let mut ids = HashSet::new();
    let mut catalogs = 0;
    for (account_id, result) in results {
        let result = result.as_ref()?;
        if let Some(status) = result.rejected_status {
            let mut data = wire::empty();
            if let Some(account_id) = account_id {
                data.set("accountId", wire::str_value(account_id.clone()));
            } else {
                data.set_undefined("accountId");
            }
            data.set("status", WireValue::Number(f64::from(status)));
            host.warn("Codex model discovery skipped an account whose credential was rejected", data);
            continue;
        }
        catalogs += 1;
        for model in &result.models {
            if ids.insert(text(model, "id").unwrap_or_else(|| "".into())) {
                models.push(model.clone());
            }
        }
    }
    if catalogs > 0 { Some(models) } else { None }
}
pub fn openai_codex_model_manager_options(
    default: &CatalogContext,
    config: OpenAiCodexModelManagerConfig,
    host: ProviderFactoryHost,
) -> ModelManagerOptions {
    let mut options = ModelManagerOptions::new("openai-codex");
    options.dynamic_models_authoritative = true;
    if let Some(resolver) = config.resolve_accounts {
        let context = config.context.unwrap_or_else(|| default.clone());
        let version = config.client_version;
        options.dynamic_fetcher = Some(dynamic(move || {
            let resolver = resolver.clone();
            let context = context.clone();
            let version = version.clone();
            let host = host.clone();
            async move {
                let Some(accounts) = resolver.resolve_accounts().await?.filter(|a| !a.is_empty()) else {
                    return Ok(RawModelValue::null());
                };
                let calls = accounts.into_iter().map(|account| {
                    let context = context.clone();
                    let version = version.clone();
                    async move {
                        let result = fetch_codex_models(
                            &context,
                            &CodexModelDiscoveryOptions {
                                access_token: account.access_token,
                                account_id: account.account_id.clone(),
                                client_version: version,
                                ..Default::default()
                            },
                        )
                        .await?;
                        Ok::<_, DiscoveryError>((account.account_id, result))
                    }
                });
                let results = promise_all(calls).await?;
                Ok(union_codex_models(&results, &host).map_or_else(RawModelValue::null, RawModelValue::models))
            }
        }));
    }
    options
}
#[derive(Clone, Default)]
pub struct CursorModelManagerConfig {
    pub api_key: Option<WireString>,
    pub base_url: Option<WireString>,
    pub client_version: Option<WireString>,
}
pub fn cursor_model_manager_options(context: &CatalogContext, config: CursorModelManagerConfig) -> ModelManagerOptions {
    let mut options = ModelManagerOptions::new("cursor");
    options.cache_provider_id = Some("cursor:default-effort-v4".into());
    if let Some(key) = config.api_key.filter(|key| !key.is_empty()) {
        let context = context.clone();
        let discovery = CursorModelDiscoveryOptions {
            api_key: key,
            base_url: config.base_url,
            client_version: config.client_version,
            ..Default::default()
        };
        options.dynamic_fetcher = Some(dynamic(move || {
            let context = context.clone();
            let discovery = discovery.clone();
            async move { RawModelValue::from_discovery(fetch_cursor_usable_models(&context, &discovery).await) }
        }));
    }
    options
}
#[derive(Clone, Debug)]
pub struct GitLabDuoModelIdentity {
    pub upstream_model_id: WireString,
    pub reference_provider: WireString,
    pub reference_model_id: WireString,
}
fn duo_identities() -> Vec<(WireString, GitLabDuoModelIdentity)> {
    [
        ("duo-chat-opus-4-6", "claude-opus-4-6", "anthropic", "claude-opus-4-6"),
        ("duo-chat-sonnet-4-6", "claude-sonnet-4-6", "anthropic", "claude-sonnet-4-6"),
        ("duo-chat-opus-4-5", "claude-opus-4-5-20251101", "anthropic", "claude-opus-4-5-20251101"),
        ("duo-chat-sonnet-4-5", "claude-sonnet-4-5-20250929", "anthropic", "claude-sonnet-4-5-20250929"),
        ("duo-chat-haiku-4-5", "claude-haiku-4-5-20251001", "anthropic", "claude-haiku-4-5-20251001"),
        ("duo-chat-gpt-5-1", "gpt-5.1-2025-11-13", "openai", "gpt-5.1"),
        ("duo-chat-gpt-5-2", "gpt-5.2-2025-12-11", "openai", "gpt-5.2"),
        ("duo-chat-gpt-5-mini", "gpt-5-mini-2025-08-07", "openai", "gpt-5-mini"),
        ("duo-chat-gpt-5-codex", "gpt-5-codex", "openai", "gpt-5-codex"),
        ("duo-chat-gpt-5-2-codex", "gpt-5.2-codex", "openai", "gpt-5.2-codex"),
    ]
    .iter()
    .map(|(alias, upstream, provider, id)| {
        (
            (*alias).into(),
            GitLabDuoModelIdentity {
                upstream_model_id: (*upstream).into(),
                reference_provider: (*provider).into(),
                reference_model_id: (*id).into(),
            },
        )
    })
    .collect()
}
pub fn resolve_gitlab_duo_model_identity(id: &WireString) -> Option<GitLabDuoModelIdentity> {
    let rows = duo_identities();
    rows.iter()
        .find(|(alias, _)| alias == id)
        .or_else(|| rows.iter().find(|(_, identity)| identity.upstream_model_id == *id))
        .map(|(_, identity)| identity.clone())
}
fn duo_display_name(alias: &WireString) -> WireString {
    let alias = alias.to_utf8().expect("fixed alias");
    let mut parts = alias["duo-chat-".len()..].split('-').peekable();
    let family = parts.next().expect("fixed family");
    let mut numeric = Vec::new();
    while parts.peek().is_some_and(|p| p.bytes().all(|b| b.is_ascii_digit())) {
        numeric.push(parts.next().expect("peeked"));
    }
    fn cap(value: &str) -> String {
        let mut chars = value.chars();
        chars.next().map_or_else(String::new, |first| first.to_uppercase().collect::<String>() + chars.as_str())
    }
    let family = if family == "gpt" { "GPT".to_owned() } else { cap(family) };
    let version = if numeric.is_empty() {
        String::new()
    } else {
        format!("{}{}", if family == "GPT" { "-" } else { " " }, numeric.join("."))
    };
    let suffix = parts.map(cap).collect::<Vec<_>>().join(" ");
    format!("Duo Chat {family}{version}{}", if suffix.is_empty() { String::new() } else { format!(" {suffix}") }).into()
}
pub fn get_gitlab_duo_models() -> Result<Vec<SpecRef>, DiscoveryError> {
    let mut models = Vec::new();
    for (alias, identity) in duo_identities() {
        let reference = wire::bundled_provider_models(&identity.reference_provider)
            .into_iter()
            .find(|model| text(model, "id").as_ref() == Some(&identity.reference_model_id))
            .ok_or_else(|| {
                DiscoveryError::new(format!(
                    "Missing bundled {}/{} reference for {}",
                    identity.reference_provider.to_utf8().unwrap(),
                    identity.reference_model_id.to_utf8().unwrap(),
                    alias.to_utf8().unwrap()
                ))
            })?;
        let route = behavior::api_route_for(&"gitlab-duo".into(), &alias)
            .and_then(|route| text(&route, "api"))
            .filter(|route| {
                ["anthropic-messages", "openai-completions", "openai-responses"]
                    .iter()
                    .any(|api| route.equals_ascii(api))
            })
            .ok_or_else(|| {
                DiscoveryError::new(format!("Missing GitLab Duo API route for {}", alias.to_utf8().unwrap()))
            })?;
        let mut spec = wire::to_model_spec(&reference);
        spec.set("id", wire::str_value(alias.clone()));
        spec.set("name", wire::str_value(duo_display_name(&alias)));
        spec.set("api", wire::str_value(route.clone()));
        spec.set("provider", wire::str_value("gitlab-duo"));
        spec.set(
            "baseUrl",
            wire::str_value(if route.equals_ascii("anthropic-messages") {
                "https://cloud.gitlab.com/ai/v1/proxy/anthropic/"
            } else {
                "https://cloud.gitlab.com/ai/v1/proxy/openai/v1"
            }),
        );
        models.push(Arc::new(WireModelPolicy.build(&spec)?));
    }
    Ok(models)
}
#[derive(Clone, Default)]
pub struct GitLabDuoWorkflowModelManagerConfig {
    pub common: ModelManagerConfig,
    pub namespace_id: Option<WireString>,
    pub project_id: Option<WireString>,
    pub cwd: Option<PathBuf>,
}
pub fn gitlab_duo_workflow_model_cache_provider_id(
    key: &WireString,
    config: &GitLabDuoWorkflowModelManagerConfig,
    host: &ProviderFactoryHost,
) -> WireString {
    let namespace = config
        .namespace_id
        .clone()
        .or_else(|| host.environment.get("GITLAB_DUO_NAMESPACE_ID"))
        .unwrap_or_else(|| "".into());
    let project = config
        .project_id
        .clone()
        .or_else(|| host.environment.get("GITLAB_DUO_PROJECT_ID"))
        .or_else(|| host.environment.get("GITLAB_DUO_PROJECT_PATH"))
        .unwrap_or_else(|| "".into());
    let cwd = config.cwd.as_ref().unwrap_or(&host.cwd);
    let cwd = WireString::from_units(cwd.as_os_str().to_string_lossy().encode_utf16().collect());
    let terms = [key.clone(), config.common.base_url.clone().unwrap_or_else(|| "".into()), namespace, project, cwd];
    let mut scope = Vec::new();
    for (index, term) in terms.iter().enumerate() {
        if index > 0 {
            scope.push(0);
        }
        scope.extend_from_slice(term.units());
    }
    format!("gitlab-duo-agent:{}", bun_hash::hash_string_base36(&WireString::from_units(scope))).into()
}
pub fn gitlab_duo_workflow_model_manager_options(
    default: &CatalogContext,
    config: GitLabDuoWorkflowModelManagerConfig,
    host: ProviderFactoryHost,
) -> ModelManagerOptions {
    let mut options = ModelManagerOptions::new("gitlab-duo-agent");
    options.dynamic_models_authoritative = true;
    options.static_models = Some(RawModelValue::models(vec![build_gitlab_duo_workflow_fallback_model(
        Some(&"claude_sonnet_4_6_vertex".into()),
        Some(&"Claude Sonnet 4.6 - Vertex".into()),
        config.common.base_url.as_ref(),
    )]));
    if let Some(key) = config.common.api_key.clone().filter(|key| !key.is_empty()) {
        options.cache_provider_id = Some(gitlab_duo_workflow_model_cache_provider_id(&key, &config, &host));
        let context = context(default, &config.common);
        let env = ["GITLAB_DUO_NAMESPACE_ID", "GITLAB_DUO_PROJECT_ID", "GITLAB_DUO_PROJECT_PATH"]
            .iter()
            .filter_map(|key| host.environment.get(key).map(|value| ((*key).to_owned(), value)))
            .collect::<HashMap<_, _>>();
        let discovery_host = Arc::new(GitLabHost::new(host.cwd.clone(), env, Arc::new(NativeGitLabFs)));
        let discovery = GitLabDiscoveryConfig {
            api_key: key,
            base_url: config.common.base_url,
            namespace_id: config.namespace_id,
            project_id: config.project_id,
            cwd: config.cwd,
            ..Default::default()
        };
        options.dynamic_fetcher = Some(dynamic(move || {
            let context = context.clone();
            let discovery = discovery.clone();
            let host = discovery_host.clone();
            async move { RawModelValue::from_discovery(fetch_gitlab_duo_workflow_models(&context, &discovery, &host).await) }
        }));
    }
    options
}
pub fn devin_model_manager_options(default: &CatalogContext, config: ModelManagerConfig) -> ModelManagerOptions {
    let mut options = ModelManagerOptions::new("devin");
    let mut static_models = static_data::fixed_models("DEVIN_STATIC_MODELS");
    if let Some(base) = config.base_url.as_ref().filter(|base| !base.equals_ascii(DEVIN_DEFAULT_BASE_URL)) {
        static_models = static_models
            .into_iter()
            .map(|model| {
                let mut out = (*model).clone();
                out.set("baseUrl", wire::str_value(base.clone()));
                Arc::new(out)
            })
            .collect();
    }
    options.static_models = Some(RawModelValue::models(static_models));
    if let Some(key) = config.api_key.clone().filter(|key| !key.is_empty()) {
        options.dynamic_models_authoritative = true;
        let context = context(default, &config);
        let discovery =
            DevinModelDiscoveryOptions { api_key: Some(key), base_url: config.base_url, ..Default::default() };
        options.dynamic_fetcher = Some(dynamic(move || {
            let context = context.clone();
            let discovery = discovery.clone();
            async move { RawModelValue::from_discovery(fetch_devin_models(&context, &discovery).await) }
        }));
    }
    options
}
pub fn zai_model_manager_options() -> ModelManagerOptions {
    ModelManagerOptions::new("zai")
}
