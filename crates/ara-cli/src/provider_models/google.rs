//! Fixed OMP provider-models/google.ts manager factories.
use super::{
    common::{context, dynamic},
    descriptor_types::ModelManagerConfig,
};
use crate::{
    catalog_discovery::{
        CatalogContext, DiscoveryError,
        google::{
            AntigravityDiscoveryOptions, GeminiCliQuotaOptions, GeminiDiscoveryOptions,
            fetch_antigravity_discovery_models, fetch_gemini_cli_quota_models, fetch_gemini_models,
        },
    },
    model_collapse::ReviewedCollapseLookup,
    model_identity_wire::{str_value, text},
    model_manager::{ModelManagerOptions, RawModelValue},
    model_wire_policy::classify_wire,
};
use ara_rpc::WireString;
use std::sync::Arc;
#[derive(Clone, Default)]
pub struct GoogleAntigravityModelManagerConfig {
    pub oauth_token: Option<WireString>,
    pub endpoint: Option<WireString>,
    pub context: Option<CatalogContext>,
}
#[derive(Clone, Default)]
pub struct GoogleGeminiCliModelManagerConfig {
    pub oauth_token: Option<WireString>,
    pub project_id: Option<WireString>,
    pub endpoint: Option<WireString>,
    pub context: Option<CatalogContext>,
}
pub fn google_model_manager_options(default: &CatalogContext, config: ModelManagerConfig) -> ModelManagerOptions {
    let mut options = ModelManagerOptions::new("google");
    options.drop_cached_model_ids_on_static_mismatch = Some(vec!["gemini-3.7-flash".into()]);
    if let Some(key) = config.api_key.clone().filter(|k| !k.is_empty()) {
        let context = context(default, &config);
        let discovery = GeminiDiscoveryOptions::new(key);
        options.dynamic_fetcher = Some(dynamic(move || {
            let context = context.clone();
            let discovery = discovery.clone();
            async move { RawModelValue::from_discovery(fetch_gemini_models(&context, &discovery).await) }
        }));
    }
    options
}
pub fn google_vertex_model_manager_options() -> ModelManagerOptions {
    let mut options = ModelManagerOptions::new("google-vertex");
    options.drop_cached_model_ids_on_static_mismatch = Some(vec!["gemini-3.7-flash".into()]);
    options
}
pub fn google_antigravity_model_manager_options(
    default: &CatalogContext,
    config: GoogleAntigravityModelManagerConfig,
) -> ModelManagerOptions {
    let mut options = ModelManagerOptions::new("google-antigravity");
    if let Some(token) = config.oauth_token.filter(|k| !k.is_empty()) {
        let context = config.context.unwrap_or_else(|| default.clone());
        let mut discovery = AntigravityDiscoveryOptions::new(token);
        discovery.endpoint = config.endpoint;
        options.dynamic_fetcher = Some(dynamic(move || {
            let context = context.clone();
            let discovery = discovery.clone();
            async move { RawModelValue::from_discovery(fetch_antigravity_discovery_models(&context, &discovery).await) }
        }));
    }
    options
}
pub fn google_gemini_cli_model_manager_options(
    default: &CatalogContext,
    config: GoogleGeminiCliModelManagerConfig,
) -> ModelManagerOptions {
    let mut options = ModelManagerOptions::new("google-gemini-cli");
    if let Some(token) = config.oauth_token.filter(|k| !k.is_empty()) {
        let context = config.context.unwrap_or_else(|| default.clone());
        let endpoint = config.endpoint.unwrap_or_else(|| "https://cloudcode-pa.googleapis.com".into());
        let project = config.project_id;
        options.dynamic_fetcher = Some(dynamic(move || {
            let context = context.clone();
            let token = token.clone();
            let endpoint = endpoint.clone();
            let project = project.clone();
            async move {
                let table = match context
                    .runtime
                    .collapse
                    .lock()
                    .expect("collapse poisoned")
                    .reviewed_collapse_table(&"google-gemini-cli".into())
                {
                    ReviewedCollapseLookup::Table(table) => table,
                    _ => return Err(DiscoveryError::new("missing reviewed collapse table for google-gemini-cli")),
                };
                let mut discovery = AntigravityDiscoveryOptions::new(token.clone());
                discovery.collapse_table = Some(table);
                let models = fetch_antigravity_discovery_models(&context, &discovery).await?;
                let Some(models) = models else {
                    let mut quota = GeminiCliQuotaOptions::new(token);
                    quota.project_id = project;
                    quota.endpoint = Some(endpoint);
                    return RawModelValue::from_discovery(fetch_gemini_cli_quota_models(&context, &quota).await);
                };
                let mut gemini = Vec::new();
                for model in models {
                    let identity = classify_wire(
                        &"google-gemini-cli".into(),
                        &text(&model, "id").unwrap_or_else(|| "".into()),
                        true,
                    )?;
                    if !text(&identity, "class").is_some_and(|class| class.equals_ascii("gemini")) {
                        continue;
                    }
                    let mut model = (*model).clone();
                    model.set("provider", str_value("google-gemini-cli"));
                    model.set("baseUrl", str_value(endpoint.clone()));
                    gemini.push(Arc::new(model));
                }
                Ok(RawModelValue::models(gemini))
            }
        }));
    }
    options
}
