//! Private Host ownership for the fixed synchronous models.yml value resolver.
//! Source: model-config-values.ts and model-registry.ts at
//! 596f2da7101178214aa27a753529d15e6b7ad91d (MIT; THIRD_PARTY_NOTICES.md).
//! A logical request owns its command operation through settlement; wire retry
//! uses the resulting lease. A configured auth refresh invalidates all sources.

use crate::model_config_values::{
    ConfigValueContext, ConfigValueResolver, HeaderConfigRecord, HeaderSource, ProcessConfigEnvironment,
    ResolveConfigValueOptions,
};
use crate::model_patch::HeaderSlot;
use crate::model_route::{AuthResolveError, RequestAuthLease, RequestAuthResolver};
use ara_ai::Model;
use async_trait::async_trait;
use std::{path::PathBuf, sync::Arc};
use tokio_util::sync::CancellationToken;

// Native synchronous load/materialization cannot interleave midway through
// provider-wide invalidation or a header-source chain.
static CONFIG_AUTH_OPERATION: std::sync::Mutex<()> = std::sync::Mutex::new(());

static CONFIG_PENDING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static CONFIG_CHANGED: tokio::sync::Notify = tokio::sync::Notify::const_new();

struct ConfigSettlement;
impl ConfigSettlement {
    fn register() -> Self {
        CONFIG_PENDING.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        Self
    }
}
impl Drop for ConfigSettlement {
    fn drop(&mut self) {
        CONFIG_PENDING.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
        CONFIG_CHANGED.notify_waiters();
    }
}

/// A normal CLI exit must await every already dispatched helper even after
/// its Run consumer drops. Second-interrupt/hard termination remains unknown.
pub async fn wait_for_config_settlement() {
    loop {
        let changed = CONFIG_CHANGED.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        if CONFIG_PENDING.load(std::sync::atomic::Ordering::Acquire) == 0 {
            return;
        }
        changed.await;
    }
}

/// Raw credential configuration is private, never model/journal metadata.
#[derive(Clone)]
pub struct ConfigRequestAuthSpec {
    pub base: RequestAuthLease,
    pub key_config: Option<String>,
    pub startup_key: Option<String>,
    pub header_sources: Vec<Vec<(String, String)>>,
    /// The static catalog has already composed the native header source tree.
    /// Plain records are literals; only a Live source resolves config values.
    /// None retains the earlier explicit-route projection used by old callers.
    pub composed_headers: Option<HeaderSlot>,
    pub invalidation_values: Vec<String>,
    pub cli_headers: Vec<(String, String)>,
    pub auth_header: bool,
    pub codex_account: bool,
}

pub struct ConfigRequestAuth {
    spec: ConfigRequestAuthSpec,
    cwd: PathBuf,
    resolver: ConfigValueResolver,
    account: Option<Arc<dyn RequestAuthResolver>>,
}

impl ConfigRequestAuth {
    pub fn new(spec: ConfigRequestAuthSpec, cwd: PathBuf, account: Option<Arc<dyn RequestAuthResolver>>) -> Self {
        Self { spec, cwd, resolver: ConfigValueResolver::new(), account }
    }

    /// Match native load-time installation only after the Host knows project cwd.
    /// Failure is cached and does not authorize old-key fallback or HTTP dispatch.
    pub async fn prepare(&self, cancel: &CancellationToken) -> Result<(), AuthResolveError> {
        let spec = self.spec.clone();
        let cwd = self.cwd.clone();
        let resolver = self.resolver.clone();
        let worker_cancel = cancel.clone();
        let settlement = ConfigSettlement::register();
        tokio::task::spawn_blocking(move || {
            let _settlement = settlement;
            let _operation = CONFIG_AUTH_OPERATION.lock().unwrap_or_else(|error| error.into_inner());
            let _warnings = ConfigWarningDrain(resolver.clone());
            if worker_cancel.is_cancelled() {
                return Err(AuthResolveError::Cancelled);
            }
            let context = ConfigValueContext { project_dir: &cwd, environment: &ProcessConfigEnvironment };
            if let Some(headers) = spec.header_sources.first() {
                for (_, config) in HeaderConfigRecord::from_pairs(headers.clone()).snapshot() {
                    if worker_cancel.is_cancelled() {
                        return Err(AuthResolveError::Cancelled);
                    }
                    resolver.resolve_config_value(&config, &context, ResolveConfigValueOptions::default());
                }
            }
            if worker_cancel.is_cancelled() {
                return Err(AuthResolveError::Cancelled);
            }
            if let Some(key) = &spec.startup_key {
                resolver.resolve_config_value(key, &context, ResolveConfigValueOptions::default());
            }
            if worker_cancel.is_cancelled() {
                return Err(AuthResolveError::Cancelled);
            }
            Ok(())
        })
        .await
        .map_err(|_| AuthResolveError::Command)?
    }

    async fn lease(
        &self,
        model: &Model,
        cancel: &CancellationToken,
        refresh: bool,
    ) -> Result<RequestAuthLease, AuthResolveError> {
        if cancel.is_cancelled() {
            return Err(AuthResolveError::Cancelled);
        }
        let base = match &self.account {
            Some(account) => account.resolve(model, cancel).await?,
            None => self.spec.base.clone(),
        };
        let spec = self.spec.clone();
        let cwd = self.cwd.clone();
        let resolver = self.resolver.clone();
        // This future is awaited through cancellation by the request owner.
        // Started helpers may have external effects; no automatic helper replay.
        let worker_cancel = cancel.clone();
        let settlement = ConfigSettlement::register();
        let result = tokio::task::spawn_blocking(move || {
            let _settlement = settlement;
            let _operation = CONFIG_AUTH_OPERATION.lock().unwrap_or_else(|error| error.into_inner());
            let _warnings = ConfigWarningDrain(resolver.clone());
            if worker_cancel.is_cancelled() {
                return Err(AuthResolveError::Cancelled);
            }
            let context = ConfigValueContext { project_dir: &cwd, environment: &ProcessConfigEnvironment };
            if refresh {
                for value in &spec.invalidation_values {
                    resolver.invalidate_command_config(Some(value));
                }
            }
            if worker_cancel.is_cancelled() {
                return Err(AuthResolveError::Cancelled);
            }
            let mut base = if let Some(key_config) = &spec.key_config {
                let key = resolver
                    .resolve_config_value(key_config, &context, ResolveConfigValueOptions::default())
                    .ok_or(AuthResolveError::Command)?;
                RequestAuthLease::new(base.identity().clone(), Some(key))
            } else {
                base
            };
            if worker_cancel.is_cancelled() {
                return Err(AuthResolveError::Cancelled);
            }
            let mut headers = Vec::new();
            if let Some(slot) = &spec.composed_headers {
                let pairs = match slot {
                    HeaderSlot::Source(HeaderSource::Config(record)) => record.snapshot(),
                    HeaderSlot::Source(HeaderSource::Live(headers)) => headers
                        .snapshot_checked(&resolver, &context, &|| !worker_cancel.is_cancelled())
                        .map_err(|_| AuthResolveError::Cancelled)?
                        .map(|value| value.into_pairs())
                        .unwrap_or_default(),
                    HeaderSlot::Absent | HeaderSlot::Undefined | HeaderSlot::Null => Vec::new(),
                };
                for (name, value) in pairs {
                    merge_header(&mut headers, name, value)?;
                }
            } else {
                for source in &spec.header_sources {
                    for (name, config) in HeaderConfigRecord::from_pairs(source.clone()).snapshot() {
                        if worker_cancel.is_cancelled() {
                            return Err(AuthResolveError::Cancelled);
                        }
                        if let Some(value) = resolver
                            .resolve_config_value(&config, &context, ResolveConfigValueOptions::default())
                            .filter(|value| !value.is_empty())
                        {
                            merge_header(&mut headers, name, value)?;
                        }
                    }
                }
            }
            if worker_cancel.is_cancelled() {
                return Err(AuthResolveError::Cancelled);
            }
            // CLI values are literals and do not re-enter config command parsing.
            for (name, value) in spec.cli_headers {
                merge_header(&mut headers, name, value)?;
            }
            if spec.auth_header
                && let Some(key) = base.api_key()
            {
                merge_header(&mut headers, "Authorization".into(), format!("Bearer {key}"))?;
            }
            base = base.extend_headers(headers);
            Ok(base)
        })
        .await
        .map_err(|_| AuthResolveError::Command)?;
        if cancel.is_cancelled() {
            return Err(AuthResolveError::Cancelled);
        }
        result
    }
}

struct ConfigWarningDrain(ConfigValueResolver);
impl Drop for ConfigWarningDrain {
    fn drop(&mut self) {
        for warning in self.0.take_warnings() {
            eprintln!("ara: model-config: !command value resolution failed (code: {})", warning.code);
        }
    }
}

fn merge_header(headers: &mut Vec<(String, String)>, name: String, value: String) -> Result<(), AuthResolveError> {
    let name = http::header::HeaderName::from_bytes(name.as_bytes()).map_err(|_| AuthResolveError::Command)?;
    http::header::HeaderValue::from_str(&value).map_err(|_| AuthResolveError::Command)?;
    headers.retain(|(old, _)| !old.eq_ignore_ascii_case(name.as_str()));
    headers.push((name.as_str().into(), value));
    Ok(())
}

#[async_trait]
impl RequestAuthResolver for ConfigRequestAuth {
    async fn resolve(&self, model: &Model, cancel: &CancellationToken) -> Result<RequestAuthLease, AuthResolveError> {
        self.lease(model, cancel, false).await
    }
    fn requires_settlement(&self) -> bool {
        true
    }
    fn supports_auth_refresh(&self) -> bool {
        !self.spec.codex_account
            && self
                .spec
                .startup_key
                .iter()
                .chain(self.spec.invalidation_values.iter())
                .chain(self.spec.header_sources.iter().flatten().map(|(_, value)| value))
                .any(|value| value.starts_with('!'))
    }
    async fn refresh(
        &self,
        model: &Model,
        cancel: &CancellationToken,
    ) -> Result<Option<RequestAuthLease>, AuthResolveError> {
        self.lease(model, cancel, true).await.map(Some)
    }
}
