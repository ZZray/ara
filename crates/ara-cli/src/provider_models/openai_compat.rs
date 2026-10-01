//! Native fixed OMP provider-models/openai-compat.ts factory and mapper surface.
pub use super::behavior::{api_route_exact_model_ids, api_route_for, is_excluded_model};
pub use super::catalog_session::{fetch_revalidated_with_timeout, fetch_well_known_models};
use super::{
    common::*,
    descriptor_types::{ModelManagerConfig, ProviderFactoryHost},
};
use crate::{
    catalog_discovery::{
        CatalogContext, DiscoveryError,
        openai::{OpenAiCompatibleOptions, OpenAiModelFilter, OpenAiModelMapper, fetch_openai_compatible_models},
    },
    model_collapse::{SpecRef, VariantSpec, lower},
    model_identity_wire::{self as wire, copy_field, text},
    model_manager::{ModelManagerOptions, RawModelValue},
};
use ara_rpc::{WireString, WireValue};
use std::{collections::HashMap, sync::Arc};
macro_rules! static_models {($($name:ident),* $(,)?)=>{$(pub static $name:std::sync::LazyLock<Vec<SpecRef>>=std::sync::LazyLock::new(||super::static_data::fixed_models(stringify!($name)));)*};}
static_models!(
    ANTHROPIC_CURATED_FALLBACK_MODELS,
    OPENAI_DAYBREAK_CURATED_FALLBACK_MODELS,
    GMI_CLOUD_STATIC_MODELS,
    XAI_OAUTH_CURATED_MODELS,
    ALIBABA_TOKEN_PLAN_STATIC_MODELS,
    META_MUSE_STATIC_MODELS,
    BEDROCK_MANTLE_STATIC_MODELS,
    SAKANA_FUGU_STATIC_MODELS,
    AIAND_STATIC_MODELS,
    ABLITERATION_STATIC_MODELS,
    YOLO_AUTO_STATIC_MODELS
);
pub type ProviderMapper = Arc<
    dyn Fn(&VariantSpec, &VariantSpec, Option<&VariantSpec>) -> Result<Option<VariantSpec>, DiscoveryError>
        + Send
        + Sync,
>;
pub type ProviderFilter =
    Arc<dyn Fn(&VariantSpec, &SpecRef, &HashMap<WireString, SpecRef>) -> Result<bool, DiscoveryError> + Send + Sync>;
pub type SimpleProviderHeadersResolver =
    Arc<dyn Fn() -> Result<Option<Vec<(WireString, WireString)>>, DiscoveryError> + Send + Sync>;
#[derive(Clone)]
pub enum SimpleProviderDiscoveryHeaders {
    Record(Vec<(WireString, WireString)>),
    Callback(SimpleProviderHeadersResolver),
}
impl SimpleProviderDiscoveryHeaders {
    fn resolve(&self) -> Result<Vec<(WireString, WireString)>, DiscoveryError> {
        match self {
            Self::Record(headers) => Ok(headers.clone()),
            Self::Callback(resolve) => Ok(resolve()?.unwrap_or_default()),
        }
    }
}
#[derive(Clone, Default)]
pub struct SimpleProviderConfig {
    pub common: ModelManagerConfig,
    pub headers: Option<SimpleProviderDiscoveryHeaders>,
}
impl From<ModelManagerConfig> for SimpleProviderConfig {
    fn from(common: ModelManagerConfig) -> Self {
        Self { common, headers: None }
    }
}
pub struct CompatibleBuilder {
    pub api: WireString,
    pub provider: WireString,
    pub default_base: WireString,
    pub config: ModelManagerConfig,
    pub headers: Vec<(WireString, WireString)>,
    pub authoritative: bool,
    pub require_api_key: bool,
    pub drop_ids: Option<Vec<WireString>>,
    pub map: ProviderMapper,
    pub filter: Option<ProviderFilter>,
}
pub fn compatible_options(default: &CatalogContext, options: CompatibleBuilder) -> ModelManagerOptions {
    compatible_options_with_headers(default, options, None)
}
fn compatible_options_with_headers(
    default: &CatalogContext,
    options: CompatibleBuilder,
    headers: Option<SimpleProviderDiscoveryHeaders>,
) -> ModelManagerOptions {
    let mut manager = ModelManagerOptions::new(options.provider.clone());
    manager.dynamic_models_authoritative = options.authoritative;
    manager.drop_cached_model_ids_on_static_mismatch = options.drop_ids;
    if options.require_api_key && options.config.api_key.as_ref().is_none_or(|key| key.is_empty()) {
        return manager;
    }
    let context = context(default, &options.config);
    let base = options.config.base_url.clone().unwrap_or(options.default_base);
    let references = Arc::new(wire::create_bundled_reference_map(&options.provider));
    let map = options.map;
    let map_references = references.clone();
    let mapper: OpenAiModelMapper = Arc::new(move |entry, defaults, _| {
        let id = text(&defaults, "id").unwrap_or_else(|| "".into());
        map(entry, &defaults, map_references.get(&id).map(|m| m.as_ref())).map(|m| m.map(Arc::new))
    });
    let filter = options.filter.map(|filter| {
        let references = references.clone();
        Arc::new(move |entry: &VariantSpec, model: &SpecRef| filter(entry, model, &references)) as OpenAiModelFilter
    });
    let mut discovery = OpenAiCompatibleOptions::new(options.api, options.provider, base);
    discovery.api_key = options.config.api_key;
    discovery.headers = options.headers;
    discovery.map_model = Some(mapper);
    discovery.filter_model = filter;
    manager.dynamic_fetcher = Some(dynamic(move || {
        let context = context.clone();
        let mut discovery = discovery.clone();
        let headers = headers.clone();
        async move {
            if let Some(headers) = headers {
                discovery.headers = headers.resolve()?;
            }
            RawModelValue::from_discovery(fetch_openai_compatible_models(&context, &discovery).await)
        }
    }));
    manager
}
fn bundled_mapper() -> ProviderMapper {
    Arc::new(|entry, defaults, reference| Ok(Some(map_with_bundled_reference(entry, defaults, reference))))
}
pub fn create_simple_openai_completions_options(
    default: &CatalogContext,
    provider: impl Into<WireString>,
    default_base: impl Into<WireString>,
    config: impl Into<SimpleProviderConfig>,
) -> ModelManagerOptions {
    let config = config.into();
    compatible_options_with_headers(
        default,
        CompatibleBuilder {
            api: "openai-completions".into(),
            provider: provider.into(),
            default_base: default_base.into(),
            config: config.common,
            headers: Vec::new(),
            authoritative: false,
            require_api_key: true,
            drop_ids: None,
            map: bundled_mapper(),
            filter: None,
        },
        config.headers,
    )
}
pub fn groq_model_manager_options(ctx: &CatalogContext, config: ModelManagerConfig) -> ModelManagerOptions {
    create_simple_openai_completions_options(ctx, "groq", "https://api.groq.com/openai/v1", config)
}
pub fn cerebras_model_manager_options(ctx: &CatalogContext, config: ModelManagerConfig) -> ModelManagerOptions {
    create_simple_openai_completions_options(ctx, "cerebras", "https://api.cerebras.ai/v1", config)
}
pub fn huggingface_model_manager_options(ctx: &CatalogContext, config: ModelManagerConfig) -> ModelManagerOptions {
    create_simple_openai_completions_options(ctx, "huggingface", "https://router.huggingface.co/v1", config)
}
pub fn nvidia_model_manager_options(ctx: &CatalogContext, config: ModelManagerConfig) -> ModelManagerOptions {
    create_simple_openai_completions_options(ctx, "nvidia", "https://integrate.api.nvidia.com/v1", config)
}
mod catalog_a;
pub use catalog_a::*;
mod models_dev;
pub use models_dev::*;
mod catalog_b;
pub use catalog_b::*;
mod catalog_c;
pub use catalog_c::*;
mod catalog_d;
pub use catalog_d::*;
mod catalog_e;
pub use catalog_e::*;
mod catalog_f;
pub use catalog_f::*;
mod catalog_g;
pub use catalog_g::*;
mod catalog_h;
pub use catalog_h::*;
mod catalog_i;
pub use catalog_i::*;
