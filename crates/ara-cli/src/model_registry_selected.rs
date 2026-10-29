//! Selected-model lazy metadata from fixed OMP 596f2da7101178214aa27a753529d15e6b7ad91d,
//! config/model-registry.ts:573-675 and config/model-discovery.ts selected probes.
//! No catalog refresh, credential resolution/refresh or bearer-key synthesis.
//! MIT; Copyright (c) 2025 Mario Zechner; 2025-2026 Can Bölük;
//! 2026 Stencil Labs, Inc. See LICENSE and THIRD_PARTY_NOTICES.md.

use super::*;
use crate::{
    catalog_discovery::DiscoverySignal,
    model_identity_wire::number,
    model_registry_discovery::{
        bounded, discovery_probe_timeout_ms, fetch_json, llama_native_base, llama_row_runtime_metadata,
        llama_server_metadata, normalize_openai_discovery_base_url,
    },
    provider_models::openai_compat::{LmStudioNativeModelMetadataOptions, fetch_lm_studio_native_model_metadata},
};

impl ModelRegistry {
    pub fn has_lazy_runtime_metadata(&self, provider: &WireString) -> bool {
        self.backend.generation().discoveries.iter().any(|config| {
            &config.provider == provider
                && config.kind().is_some_and(|kind| kind.equals_ascii("llama.cpp") || kind.equals_ascii("lm-studio"))
        })
    }

    /// Explicit Native model-selection metadata refresh. Only the selected
    /// local runtime's native endpoints are queried with the model's headers.
    /// Failed/no-op probes preserve the current model or original opaque donor.
    pub async fn refresh_selected_model_metadata(&self, model: &HostModelRef) -> RegistryRuntimeResult<HostModelRef> {
        let Some(provider) = text(model.spec(), "provider") else { return Ok(model.clone()) };
        let generation = self.backend.generation();
        let config = generation
            .discoveries
            .iter()
            .find(|config| {
                config.provider == provider
                    && config
                        .kind()
                        .is_some_and(|kind| kind.equals_ascii("llama.cpp") || kind.equals_ascii("lm-studio"))
            })
            .cloned();
        let Some(config) = config else { return Ok(model.clone()) };
        self.backend.load_standard(&generation, None)?;
        generation.engine.get_all()?;
        if self.backend.host.cancel.is_cancelled() {
            return Err(error("selected model metadata refresh cancelled"));
        }
        // Materialize only the already-selected header owner. In particular,
        // neither RegistryCredentials nor configured_key is consulted here.
        let headers = self.backend.materialize_headers(model)?.map_or_else(Vec::new, |headers| {
            headers.into_pairs().into_iter().map(|(name, value)| (name.into(), value.into())).collect()
        });
        let context = &self.backend.host.context;
        let probe = async {
            if config.kind().is_some_and(|kind| kind.equals_ascii("lm-studio")) {
                selected_lm_studio_metadata(model, &config, context, headers).await
            } else {
                selected_llama_metadata(model, &config, context, headers).await
            }
        };
        let metadata = tokio::select! {
            _ = self.backend.host.cancel.cancelled() => return Err(error("selected model metadata refresh cancelled")),
            metadata = probe => metadata,
        };
        if self.backend.host.cancel.is_cancelled() {
            return Err(error("selected model metadata refresh cancelled"));
        }
        let id = text(model.spec(), "id").ok_or_else(|| error("selected model has no catalog ID"))?;
        let current_generation = self.backend.generation();
        if !Arc::ptr_eq(&generation, &current_generation) {
            self.backend.load_standard(&current_generation, None)?;
        }
        match metadata {
            Some(metadata) => Ok(current_generation.engine.patch_selected_runtime_metadata(model, &metadata)?),
            None => Ok(current_generation.engine.find_reference(&provider, &id)?.unwrap_or_else(|| model.clone())),
        }
    }
}

async fn selected_lm_studio_metadata(
    model: &HostModelRef,
    config: &RegistryDiscoveryConfig,
    context: &CatalogContext,
    headers: Vec<(WireString, WireString)>,
) -> Option<VariantSpec> {
    let base = normalize_openai_discovery_base_url(text(model.spec(), "baseUrl").as_ref(), true);
    let signal = DiscoverySignal::default();
    let metadata = bounded(
        number(&config.discovery, "timeoutMs").unwrap_or(10000.0),
        &signal,
        fetch_lm_studio_native_model_metadata(
            context,
            &base,
            LmStudioNativeModelMetadataOptions { headers, signal: Some(signal.clone()) },
        ),
    )
    .await
    .ok()
    .flatten()?;
    let id = text(model.spec(), "id")?;
    let entry = metadata.get(&id)?;
    let mut metadata = entry.clone();
    // LM Studio has no native output cap: carry the selected model's current
    // cap only when a context window was observed, then the publisher clamps it.
    if number(entry, "contextWindow").is_some()
        && let Some(max) = number(model.spec(), "maxTokens")
    {
        metadata.set("maxTokens", WireValue::Number(max));
    }
    (!metadata.own_keys().is_empty()).then_some(metadata)
}

async fn selected_llama_metadata(
    model: &HostModelRef,
    config: &RegistryDiscoveryConfig,
    context: &CatalogContext,
    headers: Vec<(WireString, WireString)>,
) -> Option<VariantSpec> {
    let base = llama_native_base(&text(model.spec(), "baseUrl").unwrap_or_else(|| "http://127.0.0.1:8080".into()));
    let mut url = base.clone();
    url.append_str("/models");
    let models = fetch_json(
        context,
        &config.provider,
        DiscoveryRequest { url, headers: headers.clone(), ..Default::default() },
        discovery_probe_timeout_ms(&base, 250.0, number(&config.discovery, "timeoutMs")),
    );
    let server = llama_server_metadata(config, &base, &headers, context);
    let (models, server) = tokio::join!(models, server);
    let models = models.ok()?;
    if !matches!(models.value, WireValue::Object(_)) {
        return None;
    }
    let id = text(model.spec(), "id")?;
    let entry = models.get("data")?.as_array()?.iter().find_map(|entry| {
        if !matches!(entry, WireValue::Object(_)) {
            return None;
        }
        let entry = VariantSpec::from_wire(entry.clone());
        (text(&entry, "id").as_ref() == Some(&id)).then_some(entry)
    })?;
    let metadata = llama_row_runtime_metadata(&entry, server.as_ref());
    (!metadata.own_keys().is_empty()).then_some(metadata)
}
