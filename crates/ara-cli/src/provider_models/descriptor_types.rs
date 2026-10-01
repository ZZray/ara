//! Fixed OMP provider-models/descriptor-types.ts, Host supplied dependencies.
use crate::{
    catalog_discovery::{CatalogContext, DiscoveryError},
    model_manager::ModelManagerOptions,
};
use ara_rpc::WireString;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

type FactoryCallback = dyn Fn(&CatalogContext, ModelManagerConfig, &ProviderFactoryHost) -> Result<ModelManagerOptions, DiscoveryError>
    + Send
    + Sync;
/// Stable native callback identity shared by the catalog and runtime registry.
#[derive(Clone)]
pub struct ModelManagerFactory(Arc<FactoryCallback>);
impl ModelManagerFactory {
    pub fn new(
        callback: impl Fn(
            &CatalogContext,
            ModelManagerConfig,
            &ProviderFactoryHost,
        ) -> Result<ModelManagerOptions, DiscoveryError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self(Arc::new(callback))
    }
    pub fn create(
        &self,
        context: &CatalogContext,
        config: ModelManagerConfig,
        host: &ProviderFactoryHost,
    ) -> Result<ModelManagerOptions, DiscoveryError> {
        (self.0)(context, config, host)
    }
    pub fn same_identity(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl std::fmt::Debug for ModelManagerFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ModelManagerFactory")
    }
}

#[derive(Clone, Default)]
pub struct ModelManagerConfig {
    pub api_key: Option<WireString>,
    pub base_url: Option<WireString>,
    pub authenticated: bool,
    /// Overrides the default Host transport for this factory's fetch identity.
    pub context: Option<CatalogContext>,
}
#[derive(Clone, Debug)]
pub struct CatalogDiscoveryConfig {
    pub label: WireString,
    pub env_vars: Option<Vec<WireString>>,
    pub oauth_provider: Option<WireString>,
    pub allow_unauthenticated: Option<bool>,
}
#[derive(Clone, Debug)]
pub struct ProviderCatalogEntry {
    pub id: WireString,
    pub default_model: WireString,
    pub env_vars: Option<Vec<WireString>>,
    pub has_factory: bool,
    pub create_model_manager_options: Option<ModelManagerFactory>,
    pub allow_unauthenticated: Option<bool>,
    pub dynamic_models_authoritative: Option<bool>,
    pub catalog_discovery: Option<CatalogDiscoveryConfig>,
    pub special_model_manager: Option<bool>,
}
#[derive(Clone, Debug)]
pub struct ProviderDescriptor {
    pub provider_id: WireString,
    pub default_model: WireString,
    pub create_model_manager_options: ModelManagerFactory,
    pub allow_unauthenticated: Option<bool>,
    pub dynamic_models_authoritative: Option<bool>,
    pub catalog_discovery: Option<CatalogDiscoveryConfig>,
}
pub fn is_catalog_descriptor(value: &ProviderDescriptor) -> bool {
    value.catalog_discovery.is_some()
}
pub fn allows_unauthenticated_catalog_discovery(value: &ProviderDescriptor) -> bool {
    value
        .catalog_discovery
        .as_ref()
        .and_then(|d| d.allow_unauthenticated)
        .or(value.allow_unauthenticated)
        .unwrap_or(false)
}
pub trait ProviderEnvironment: Send + Sync {
    fn get(&self, key: &str) -> Option<WireString>;
}
impl<F: Fn(&str) -> Option<WireString> + Send + Sync> ProviderEnvironment for F {
    fn get(&self, key: &str) -> Option<WireString> {
        self(key)
    }
}
#[derive(Clone)]
pub struct ProviderLogEntry {
    pub level: &'static str,
    pub message: WireString,
    pub data: crate::model_collapse::VariantSpec,
}
#[derive(Clone)]
pub struct ProviderFactoryHost {
    pub environment: Arc<dyn ProviderEnvironment>,
    pub cwd: PathBuf,
    pub user_agent: WireString,
    pub sessions: Arc<super::catalog_session::CatalogSessions>,
    pub logs: Arc<Mutex<Vec<ProviderLogEntry>>>,
}
impl ProviderFactoryHost {
    pub fn new(environment: Arc<dyn ProviderEnvironment>, user_agent: impl Into<WireString>, cwd: PathBuf) -> Self {
        Self {
            environment,
            cwd,
            user_agent: user_agent.into(),
            sessions: Arc::new(super::catalog_session::CatalogSessions::default()),
            logs: Arc::new(Mutex::new(Vec::new())),
        }
    }
    pub fn warn(&self, message: impl Into<WireString>, data: crate::model_collapse::VariantSpec) {
        self.logs.lock().expect("provider logs poisoned").push(ProviderLogEntry {
            level: "warn",
            message: message.into(),
            data,
        });
    }
}
