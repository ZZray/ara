//! Registry replay calls the actual catalog/runtime factory handles.
use super::*;
use ara_cli::provider_models::{descriptor_types::*, descriptors::*};
fn optional_bool(value: Option<bool>) -> W {
    value.map(W::Bool).unwrap_or(W::Null)
}
fn optional_strings(value: Option<&[WireString]>) -> W {
    value.map(|v| W::Array(v.iter().cloned().map(s).collect())).unwrap_or(W::Null)
}
fn discovery(value: Option<&CatalogDiscoveryConfig>) -> W {
    value
        .map(|v| {
            W::object(vec![
                ("label", s(v.label.clone())),
                ("envVars", optional_strings(v.env_vars.as_deref())),
                ("oauthProvider", v.oauth_provider.clone().map(s).unwrap_or(W::Null)),
                ("allowUnauthenticated", optional_bool(v.allow_unauthenticated)),
            ])
        })
        .unwrap_or(W::Null)
}
fn registry() -> VariantSpec {
    let catalog = catalog_providers()
        .iter()
        .map(|v| {
            W::object(vec![
                ("id", s(v.id.clone())),
                ("defaultModel", s(v.default_model.clone())),
                ("envVars", optional_strings(v.env_vars.as_deref())),
                ("hasFactory", W::Bool(v.create_model_manager_options.is_some())),
                ("allowUnauthenticated", optional_bool(v.allow_unauthenticated)),
                ("dynamicModelsAuthoritative", optional_bool(v.dynamic_models_authoritative)),
                ("catalogDiscovery", discovery(v.catalog_discovery.as_ref())),
                ("specialModelManager", optional_bool(v.special_model_manager)),
                ("lookupSame", W::Bool(std::ptr::eq(v, get_catalog_provider_entry(&v.id).unwrap()))),
            ])
        })
        .collect();
    let descriptors = provider_descriptors();
    let runtime = descriptors
        .iter()
        .map(|v| {
            W::object(vec![
                ("providerId", s(v.provider_id.clone())),
                ("defaultModel", s(v.default_model.clone())),
                ("allowUnauthenticated", optional_bool(v.allow_unauthenticated)),
                ("dynamicModelsAuthoritative", optional_bool(v.dynamic_models_authoritative)),
                ("catalogDiscovery", discovery(v.catalog_discovery.as_ref())),
                (
                    "factorySame",
                    W::Bool(
                        v.create_model_manager_options.same_identity(
                            get_catalog_provider_entry(&v.provider_id)
                                .unwrap()
                                .create_model_manager_options
                                .as_ref()
                                .unwrap(),
                        ),
                    ),
                ),
                ("isCatalog", W::Bool(is_catalog_descriptor(v))),
                (
                    "allowsUnauthenticated",
                    if is_catalog_descriptor(v) {
                        W::Bool(allows_unauthenticated_catalog_discovery(v))
                    } else {
                        W::Null
                    },
                ),
            ])
        })
        .collect();
    let mut precedence = Vec::new();
    for outer in [None, Some(false), Some(true)] {
        for inner in [None, Some(false), Some(true)] {
            let mut d = descriptors[0].clone();
            d.allow_unauthenticated = outer;
            d.catalog_discovery = Some(CatalogDiscoveryConfig {
                label: "Fixture".into(),
                env_vars: None,
                oauth_provider: None,
                allow_unauthenticated: inner,
            });
            precedence.push(W::Bool(allows_unauthenticated_catalog_discovery(&d)));
        }
    }
    VariantSpec::from_wire(W::object(vec![
        ("catalog", W::Array(catalog)),
        ("runtime", W::Array(runtime)),
        ("defaults", W::Object(default_model_per_provider().into_iter().map(|(k, v)| (k, s(v))).collect())),
        (
            "unknownLookup",
            get_catalog_provider_entry(&"fixture-unknown".into()).map(|_| W::Bool(true)).unwrap_or(W::Null),
        ),
        ("precedence", W::Array(precedence)),
    ]))
}
pub async fn replay(test: &W, source_cwd: PathBuf) -> W {
    let transport = Arc::new(FixtureTransport::new(test));
    let cursor = matches!(test.get("cursor"), Some(W::Bool(true)));
    let server = if cursor { Some(rpc_fixture::start_rpc_fixture_server(test).await) } else { None };
    let context = if cursor {
        CatalogContext::new(Arc::new(NativeDiscoveryTransport::with_extra_ca(None).unwrap())).unwrap()
    } else {
        CatalogContext::new(transport.clone()).unwrap()
    };
    if text(field(test, "op")) == "registry" {
        return manager_fixture::finish(&transport, &context, success(&registry()));
    }
    let env: HashMap<_, _> = pairs(test.get("env")).into_iter().map(|(k, v)| (k.to_utf8().unwrap(), v)).collect();
    let host = ProviderFactoryHost::new(Arc::new(move |key: &str| env.get(key).cloned()), "omp/18.1.8", source_cwd);
    let options = field(test, "options");
    let config = ModelManagerConfig {
        api_key: options.get("apiKey").and_then(W::as_string).cloned(),
        base_url: server
            .as_ref()
            .map(|server| format!("http://127.0.0.1:{}", server.port).into())
            .or_else(|| options.get("baseUrl").and_then(W::as_string).cloned()),
        authenticated: matches!(options.get("authenticated"), Some(W::Bool(true))),
        // The source Cursor factory ignores config.fetch; keep the injected
        // transport distinct from its default native H2 context.
        context: Some(if cursor { CatalogContext::new(transport.clone()).unwrap() } else { context.clone() }),
    };
    let id = string(test, "provider");
    let descriptors = provider_descriptors();
    let callback = descriptors
        .iter()
        .find(|row| row.provider_id == id)
        .map(|row| &row.create_model_manager_options)
        .unwrap_or_else(|| get_catalog_provider_entry(&id).unwrap().create_model_manager_options.as_ref().unwrap());
    let built = match callback.create(&context, config, &host) {
        Ok(built) => built,
        Err(error) => return manager_fixture::finish(&transport, &context, failure(error)),
    };
    let result = match manager_fixture::encode_options(built, test).await {
        Ok(mut result) => {
            if let Some(server) = &server {
                let receipt = server.read_receipts(false).await;
                result.set("protocol", W::object(vec![("requests", field(&receipt, "requests").clone())]));
                rpc_fixture::normalize_rpc_port(&mut result.value, &server.port.to_string());
            }
            success(&result)
        }
        Err(error) => failure(error),
    };
    manager_fixture::finish(&transport, &context, result)
}
