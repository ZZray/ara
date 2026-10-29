//! Fixed OMP 596f2da append-only-context-mode.ts and AgentSession setting signal.
//! MIT; Copyright (c) 2025 Mario Zechner; (c) 2025-2026 Can B鏋歭鐪塳;
//! Copyright (c) 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md.
//!
//! This Host owns sparse selected-model metadata, runtime settings and Session
//! subscriptions. Core receives only an owned portable context binding.

use ara_agent::{
    AppendOnlyContextManager, BuildOptions, ExecutionSnapshot, ProviderContextBinding, SharedAppendOnlyContextManager,
};
use ara_ai::Model;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, RwLock, Weak,
        atomic::{AtomicU64, Ordering},
    },
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppendOnlyContextMode {
    #[default]
    Auto,
    On,
    Off,
}

impl AppendOnlyContextMode {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "auto" => Ok(Self::Auto),
            "on" => Ok(Self::On),
            "off" => Ok(Self::Off),
            _ => Err(format!("unknown append-only context mode {value:?} (auto, on, off)")),
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::On => "on",
            Self::Off => "off",
        }
    }
}

/// Verbatim authored compat only: resolved provider defaults are not user intent.
#[derive(Clone, Debug)]
pub struct AppendOnlyContextModel {
    pub provider: String,
    pub base_url: String,
    pub compat_config: Option<Value>,
}

impl AppendOnlyContextModel {
    pub fn from_selected(model: &Model, metadata: Option<&Value>) -> Self {
        Self {
            provider: model.provider.clone(),
            base_url: model.base_url.clone(),
            compat_config: metadata.and_then(|value| value.get("compatConfig")).cloned(),
        }
    }
}

fn has_local_base_url(base_url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(base_url) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    if matches!(host.as_str(), "localhost" | "127.0.0.1" | "0.0.0.0" | "::1" | "[::1]")
        || host.starts_with("10.")
        || host.starts_with("192.168.")
        || host.ends_with(".local")
    {
        return true;
    }
    let mut pieces = host.split('.');
    pieces.next() == Some("172")
        && pieces
            .next()
            .is_some_and(|octet| octet.len() == 2 && octet.parse::<u8>().is_ok_and(|value| (16..=31).contains(&value)))
}

pub fn should_enable_append_only_context(mode: AppendOnlyContextMode, model: Option<&AppendOnlyContextModel>) -> bool {
    match mode {
        AppendOnlyContextMode::On => true,
        AppendOnlyContextMode::Off => false,
        AppendOnlyContextMode::Auto => model.is_some_and(|model| {
            matches!(model.provider.as_str(), "deepseek" | "ollama" | "ollama-cloud" | "lm-studio" | "llama.cpp")
                || crate::model_identity::host_matches_url(&model.base_url, "xiaomi")
                || has_local_base_url(&model.base_url)
                || model.compat_config.as_ref().and_then(|value| value.get("supportsStore")) == Some(&Value::Bool(true))
        }),
    }
}

/// Native parseFlag is case-sensitive and does not trim arbitrary env values.
pub fn parse_intent_flag(value: Option<&str>, default: bool) -> bool {
    match value {
        None | Some("") => default,
        Some("1" | "Y" | "y" | "TRUE" | "true" | "YES" | "yes" | "ON" | "on") => true,
        Some(_) => false,
    }
}

#[derive(Clone, Debug)]
pub struct HostContextPreferences {
    pub mode: AppendOnlyContextMode,
    pub intent_tracing: bool,
    pub inline_descriptors: AppendOnlyContextMode,
    pub tool_format: String,
}

impl Default for HostContextPreferences {
    fn default() -> Self {
        Self {
            mode: AppendOnlyContextMode::Auto,
            intent_tracing: true,
            inline_descriptors: AppendOnlyContextMode::Auto,
            tool_format: "auto".into(),
        }
    }
}

impl HostContextPreferences {
    /// Resolve once against the Session-start model. Mid-session model adoption
    /// must preserve this prune decision and the original env snapshot.
    pub fn build_options(
        &self,
        model_id: &str,
        intent_env: Option<&str>,
        no_intent_env: Option<&str>,
    ) -> Result<BuildOptions, String> {
        let prune = match self.inline_descriptors {
            AppendOnlyContextMode::On => true,
            AppendOnlyContextMode::Off => false,
            AppendOnlyContextMode::Auto => {
                let identity = crate::catalog_rules::classify_model(
                    "",
                    model_id,
                    crate::catalog_rules::ClassifyOptions { lenient: true, observed_at_ms: None },
                )
                .map_err(|error| error.to_string())?;
                identity.get("class").and_then(Value::as_str) == Some("gemini")
            }
        };
        Ok(BuildOptions {
            intent_tracing: parse_intent_flag(intent_env, self.intent_tracing),
            prune_tool_descriptions: Some(prune),
            suppress_intent_schema: no_intent_env == Some("1"),
        })
    }
    pub fn owned_dialect(&self, metadata: Option<&Value>) -> bool {
        match self.tool_format.as_str() {
            "native" => false,
            "auto" => metadata.and_then(|value| value.get("supportsTools")) == Some(&Value::Bool(false)),
            _ => true,
        }
    }
}

/// Implemented by the reference Host's existing native YAML settings owner.
/// A failed store/reload never publishes a partially read mode to subscribers.
pub trait AppendOnlyContextSettingsStore: Send + Sync {
    fn load(&self) -> Result<HostContextPreferences, String>;
    fn store_mode(&self, mode: AppendOnlyContextMode) -> Result<(), String>;
}

struct SettingsState {
    mode: AppendOnlyContextMode,
    subscribers: BTreeMap<u64, Weak<ControllerInner>>,
}

pub struct AppendOnlyContextSettings {
    store: Arc<dyn AppendOnlyContextSettingsStore>,
    operation: Mutex<()>,
    state: Mutex<SettingsState>,
}

impl AppendOnlyContextSettings {
    pub fn new(store: Arc<dyn AppendOnlyContextSettingsStore>) -> Result<Arc<Self>, String> {
        let mode = store.load()?.mode;
        Ok(Arc::new(Self {
            store,
            operation: Mutex::new(()),
            state: Mutex::new(SettingsState { mode, subscribers: BTreeMap::new() }),
        }))
    }
    pub fn get(&self) -> AppendOnlyContextMode {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).mode
    }
    pub fn preferences(&self) -> Result<HostContextPreferences, String> {
        self.store.load()
    }
    pub fn set(&self, mode: AppendOnlyContextMode) -> Result<(), String> {
        let _operation = self.operation.lock().unwrap_or_else(|e| e.into_inner());
        self.store.store_mode(mode)?;
        self.publish_mode(mode);
        Ok(())
    }
    pub fn reload(&self) -> Result<AppendOnlyContextMode, String> {
        let _operation = self.operation.lock().unwrap_or_else(|e| e.into_inner());
        let mode = self.store.load()?.mode;
        self.publish_mode(mode);
        Ok(mode)
    }
    fn publish_mode(&self, mode: AppendOnlyContextMode) {
        let subscribers = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.mode == mode {
                return;
            }
            state.mode = mode;
            state.subscribers.retain(|_, subscriber| subscriber.strong_count() != 0);
            state.subscribers.values().filter_map(Weak::upgrade).collect::<Vec<_>>()
        };
        for subscriber in subscribers {
            subscriber.reconcile(mode, None);
        }
    }
    pub fn subscription_count(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .subscribers
            .values()
            .filter(|subscriber| subscriber.strong_count() != 0)
            .count()
    }
}

static NEXT_CONTROLLER_ID: AtomicU64 = AtomicU64::new(1);
struct ControllerState {
    model: AppendOnlyContextModel,
    resolution: Option<(bool, String)>,
    binding: ProviderContextBinding,
    publication: Option<Arc<SnapshotPublication>>,
    tool_format: String,
}
struct ControllerInner {
    id: u64,
    settings: Arc<AppendOnlyContextSettings>,
    state: Mutex<ControllerState>,
}

impl Drop for ControllerInner {
    fn drop(&mut self) {
        self.settings.state.lock().unwrap_or_else(|e| e.into_inner()).subscribers.remove(&self.id);
    }
}

fn reconcile_state(state: &mut ControllerState, mode: AppendOnlyContextMode) -> bool {
    let resolution = (should_enable_append_only_context(mode, Some(&state.model)), state.model.provider.clone());
    if state.resolution.as_ref() == Some(&resolution) {
        return false;
    }
    state.resolution = Some(resolution.clone());
    if resolution.0 {
        if let Some(manager) = &state.binding.append_only_context {
            manager.lock().unwrap_or_else(|e| e.into_inner()).invalidate_for_model_change();
        } else {
            state.binding.append_only_context = Some(Arc::new(Mutex::new(AppendOnlyContextManager::new())));
        }
    } else {
        state.binding.append_only_context = None;
    }
    true
}

fn selected_state(state: &mut ControllerState, model: &Model, metadata: Option<&Value>) {
    state.model = AppendOnlyContextModel::from_selected(model, metadata);
    state.binding.owned_dialect = match state.tool_format.as_str() {
        "native" => false,
        "auto" => metadata.and_then(|value| value.get("supportsTools")) == Some(&Value::Bool(false)),
        _ => true,
    };
}

impl ControllerInner {
    fn reconcile(&self, mode: AppendOnlyContextMode, selected: Option<(&Model, Option<&Value>)>) {
        let publication = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let old_owned = state.binding.owned_dialect;
            if let Some((model, metadata)) = selected {
                selected_state(&mut state, model, metadata);
            }
            if !reconcile_state(&mut state, mode) && old_owned == state.binding.owned_dialect {
                return;
            }
            state.publication.clone()
        };
        if let Some(publication) = publication {
            publication.refresh(self);
        }
    }
}

/// Shared serial publication fence: an old controller callback cannot write a
/// newly adopted Session's snapshot even while the old Run still owns its Arc.
pub struct SnapshotPublication {
    state: Mutex<(u64, Weak<RwLock<ExecutionSnapshot>>, Weak<ControllerInner>)>,
}

impl SnapshotPublication {
    pub fn new(snapshot: &Arc<RwLock<ExecutionSnapshot>>) -> Arc<Self> {
        Arc::new(Self { state: Mutex::new((0, Arc::downgrade(snapshot), Weak::new())) })
    }
    fn refresh(&self, controller: &ControllerInner) {
        let owner = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if owner.0 != controller.id {
            return;
        }
        if let Some(snapshot) = owner.1.upgrade() {
            let binding = controller.state.lock().unwrap_or_else(|e| e.into_inner()).binding.clone();
            snapshot.write().unwrap_or_else(|e| e.into_inner()).provider_context = Some(binding);
        }
    }
    /// Prepare first, then publish route and matching binding under one fence.
    pub fn publish(
        self: &Arc<Self>,
        controller: &SessionContextController,
        mut snapshot: ExecutionSnapshot,
        metadata: Option<&Value>,
    ) {
        let _operation = controller.inner.settings.operation.lock().unwrap_or_else(|e| e.into_inner());
        let mode = controller.inner.settings.get();
        let mut owner = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut current = controller.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(selected) = &snapshot.model {
            selected_state(&mut current, &selected.model, metadata);
        }
        reconcile_state(&mut current, mode);
        current.publication = Some(self.clone());
        snapshot.provider_context = Some(current.binding.clone());
        owner.0 = controller.inner.id;
        owner.2 = Arc::downgrade(&controller.inner);
        if let Some(target) = owner.1.upgrade() {
            *target.write().unwrap_or_else(|e| e.into_inner()) = snapshot;
        }
    }
    pub fn set_append_only_context(
        &self,
        manager: Option<SharedAppendOnlyContextManager>,
    ) -> Result<(), ara_agent::ProviderContextError> {
        let owner = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let controller = owner.2.upgrade().ok_or(ara_agent::ProviderContextError::HostOwnedMutation)?;
        let mut current = controller.state.lock().unwrap_or_else(|e| e.into_inner());
        current.binding.append_only_context = manager;
        if let Some(target) = owner.1.upgrade() {
            target.write().unwrap_or_else(|e| e.into_inner()).provider_context = Some(current.binding.clone());
        }
        Ok(())
    }
    pub fn publish_unbound(&self, snapshot: ExecutionSnapshot) {
        let mut owner = self.state.lock().unwrap_or_else(|e| e.into_inner());
        owner.0 = 0;
        owner.2 = Weak::new();
        if let Some(target) = owner.1.upgrade() {
            *target.write().unwrap_or_else(|e| e.into_inner()) = snapshot;
        }
    }
}

#[derive(Clone)]
pub struct SessionContextController {
    inner: Arc<ControllerInner>,
}

impl SessionContextController {
    pub fn new(
        settings: Arc<AppendOnlyContextSettings>,
        model: &Model,
        metadata: Option<&Value>,
        build_options: BuildOptions,
        tool_format: &str,
    ) -> Self {
        let owned_dialect = match tool_format {
            "native" => false,
            "auto" => metadata.and_then(|value| value.get("supportsTools")) == Some(&Value::Bool(false)),
            _ => true,
        };
        let _operation = settings.operation.lock().unwrap_or_else(|e| e.into_inner());
        let inner = Arc::new(ControllerInner {
            id: NEXT_CONTROLLER_ID.fetch_add(1, Ordering::Relaxed),
            settings: settings.clone(),
            state: Mutex::new(ControllerState {
                model: AppendOnlyContextModel::from_selected(model, metadata),
                resolution: None,
                binding: ProviderContextBinding { build_options, owned_dialect, ..Default::default() },
                publication: None,
                tool_format: tool_format.into(),
            }),
        });
        let mut setting = settings.state.lock().unwrap_or_else(|e| e.into_inner());
        let mode = setting.mode;
        setting.subscribers.insert(inner.id, Arc::downgrade(&inner));
        drop(setting);
        inner.reconcile(mode, None);
        Self { inner }
    }
    pub fn binding(&self) -> ProviderContextBinding {
        self.inner.state.lock().unwrap_or_else(|e| e.into_inner()).binding.clone()
    }
    /// Bind an unpublished candidate once. Published controllers only validate
    /// an identical selection; a failed candidate never changes active lineage.
    pub fn stage_actual_values(
        &self,
        values: Option<Arc<ara_agent::SessionValues>>,
    ) -> Result<(), ara_agent::ProviderContextError> {
        let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        let identical = match (&state.binding.actual_values, &values) {
            (None, None) => true,
            (Some(current), Some(selected)) => Arc::ptr_eq(current, selected),
            _ => false,
        };
        if identical {
            return Ok(());
        }
        if state.publication.is_none() && state.binding.actual_values.is_none() {
            state.binding.actual_values = values;
            return Ok(());
        }
        Err(ara_agent::ProviderContextError::HostOwnedMutation)
    }
    /// Stage the Session realm and its actual transport port together before
    /// publication. Existing identical bindings are validation only; neither
    /// a published nor an already selected owner can be replaced here.
    pub fn stage_actual_binding(
        &self,
        values: Arc<ara_agent::SessionValues>,
        port: Arc<dyn ara_agent::OwnedProviderContextPort>,
    ) -> Result<(), ara_agent::ProviderContextError> {
        let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        let same_values = state.binding.actual_values.as_ref().is_some_and(|current| Arc::ptr_eq(current, &values));
        let same_port = state.binding.owned_port.as_ref().is_some_and(|current| Arc::ptr_eq(current, &port));
        if same_values && same_port {
            return Ok(());
        }
        if state.publication.is_none()
            && (state.binding.actual_values.is_none() || same_values)
            && (state.binding.owned_port.is_none() || same_port)
        {
            state.binding.actual_values = Some(values);
            state.binding.owned_port = Some(port);
            return Ok(());
        }
        Err(ara_agent::ProviderContextError::HostOwnedMutation)
    }
    pub fn tool_format(&self) -> String {
        self.inner.state.lock().unwrap_or_else(|e| e.into_inner()).tool_format.clone()
    }
    pub fn settings(&self) -> &Arc<AppendOnlyContextSettings> {
        &self.inner.settings
    }
    /// Call only after route/journal publication succeeds, never for a candidate.
    pub fn publish_model(&self, model: &Model, metadata: Option<&Value>) {
        let _operation = self.inner.settings.operation.lock().unwrap_or_else(|e| e.into_inner());
        self.inner.reconcile(self.inner.settings.get(), Some((model, metadata)));
    }
    /// Explicit provider-session/replay boundary. Ordinary usage epochs do not call this.
    pub fn fresh_provider_session(&self) {
        if let Some(manager) = self.binding().append_only_context {
            manager.lock().unwrap_or_else(|e| e.into_inner()).invalidate_for_model_change();
        }
    }
    pub fn set_append_only_context(&self, manager: Option<SharedAppendOnlyContextManager>) {
        let publication = {
            let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            state.binding.append_only_context = manager;
            state.publication.clone()
        };
        if let Some(publication) = publication {
            publication.refresh(&self.inner);
        }
    }
}
