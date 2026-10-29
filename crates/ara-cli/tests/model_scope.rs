//! Fixed source contract and frozen Windows Bun 1.4.0 glob oracle.
//! Native SHA 596f2da7101178214aa27a753529d15e6b7ad91d, model-resolver.ts
//! 114-203,700-719,1694-1780,1830-1865. Raw generator/output are retained at
//! C:/Temp/ara-model-fallback-batch/model-scope-oracle.{mjs,json}.
//! That oracle covers glob matching/suffix flags, not the full cycling function.
//! Startup mapping/empty-scope discovery follows main.ts:815-860 and the Native
//! main-rebuild-scoped-models.test.ts family through a real Registry facade.
//! Rust execution belongs to the parent gate; this file is not a test receipt.

use ara_cli::{
    catalog_discovery::{CatalogContext, DiscoveryError, DiscoveryReply, DiscoveryRequest, DiscoveryTransport},
    model_collapse::{CollapseRuntime, VariantSpec},
    model_config_file::ModelsConfigFile,
    model_config_values::{
        ConfigValueResolver, HeaderConfigRecord, HeaderResolutionOptions, HeaderSource, create_live_config_headers,
    },
    model_identity_wire::text,
    model_patch::{HeaderSlot, HostModel, HostModelRef, OrderedProviderSet},
    model_registry::{EnvironmentRegistryCredentials, ModelRegistry, ModelRegistryHost},
    model_registry_loader::{RegistryLoadOptions, startup_model_cache_provider_ids},
    model_roles::{ModelMatchPreferences, ModelRoleLookup, ModelRoleResolver},
    model_scope::{
        ScopedModel, SessionScopedModel, filter_available_models_by_enabled_patterns as filter,
        resolve_current_scoped_models, resolve_model_scope, resolve_scoped_models, same_scoped_model_set,
        to_session_scoped_models,
    },
    provider_models::{ProviderEnvironment, ProviderFactoryHost},
    retry_fallback::{ConfiguredThinkingLevel, ThinkingLevel},
};
#[test]
fn selected_routed_selector_uses_catalog_id_and_actual_host() {
    use ara_cli::model_roles::format_model_string_with_routing;
    for (provider, url, key, only, expected) in [
        ("custom", "https://openrouter.ai/api/v1", "openRouterRouting", json!(["cerebras"]), "custom/catalog@cerebras"),
        (
            "custom",
            "https://ai-gateway.vercel.sh/v1",
            "vercelGatewayRouting",
            json!(["anthropic"]),
            "custom/catalog@anthropic",
        ),
        (
            "openrouter",
            "https://ai-gateway.vercel.sh/v1",
            "openRouterRouting",
            json!(["cerebras"]),
            "openrouter/catalog@cerebras",
        ),
        ("openrouter", "https://example.invalid/v1", "openRouterRouting", json!(["a", "b"]), "openrouter/catalog"),
        ("custom", "https://example.invalid/v1", "openRouterRouting", json!(["a"]), "custom/catalog"),
    ] {
        let model = ara_cli::model_patch::build_host_model(
            &VariantSpec::from_json(&json!({
                "provider":provider,"id":"catalog","name":"Catalog","api":"openai-completions",
                "reasoning":false,"contextWindow":131072,"maxTokens":8192,"input":["text"],
                "requestModelId":"wire-only","baseUrl":url,
                "compat":{key:{"only":only}}
            })),
            HeaderSlot::Undefined,
        )
        .unwrap();
        assert_eq!(format_model_string_with_routing(&model).to_utf8().unwrap(), expected);
    }
}
#[cfg(windows)]
use ara_rpc::{WireString, WireValue};
use async_trait::async_trait;
use serde_json::json;
use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

fn model(provider: &str, id: &str, index: usize) -> HostModelRef {
    HostModel::new(
        Arc::new(VariantSpec::from_json(&json!({"provider":provider,"id":id,"name":id,"index":index}))),
        HeaderSlot::Undefined,
    )
    .unwrap()
}
fn resolver(models: Vec<HostModelRef>, preferences: ModelMatchPreferences) -> ModelRoleResolver {
    ModelRoleResolver::new(models, Arc::new(Mutex::new(CollapseRuntime::new().unwrap())), preferences).unwrap()
}
fn scopes(resolver: &ModelRoleResolver, patterns: &[&str]) -> Vec<HostModelRef> {
    filter(resolver, &patterns.iter().map(|pattern| (*pattern).into()).collect::<Vec<_>>(), &|_role: &str| None)
        .unwrap()
}
fn cycling_scopes(resolver: &ModelRoleResolver, patterns: &[&str], settings: &dyn ModelRoleLookup) -> Vec<ScopedModel> {
    resolve_model_scope(resolver, &patterns.iter().map(|pattern| (*pattern).into()).collect::<Vec<_>>(), settings)
        .unwrap()
}
fn reasoning_model(provider: &str, id: &str, index: usize, efforts: &[&str]) -> HostModelRef {
    HostModel::new(
        Arc::new(VariantSpec::from_json(&json!({"provider":provider,"id":id,"name":id,"index":index,
            "reasoning":true,"thinking":{"mode":"effort","efforts":efforts}}))),
        HeaderSlot::Undefined,
    )
    .unwrap()
}
fn cycling_observations(scoped: &[ScopedModel]) -> Vec<(String, Option<ThinkingLevel>, bool)> {
    scoped
        .iter()
        .map(|entry| {
            (
                format!(
                    "{}/{}",
                    text(entry.model.spec(), "provider").unwrap().to_utf8().unwrap(),
                    text(entry.model.spec(), "id").unwrap().to_utf8().unwrap()
                ),
                entry.thinking_level,
                entry.explicit_thinking_level,
            )
        })
        .collect()
}
fn indexes(models: &[HostModelRef]) -> Vec<usize> {
    models.iter().map(|model| model.spec().get("index").unwrap().as_number().unwrap() as usize).collect()
}
fn ids(models: &[HostModelRef]) -> Vec<String> {
    models.iter().map(|model| text(model.spec(), "id").unwrap().to_utf8().unwrap()).collect()
}

#[derive(Default)]
struct ScopeDiscovery {
    calls: AtomicUsize,
}
#[async_trait]
impl DiscoveryTransport for ScopeDiscovery {
    fn context_id(&self) -> u64 {
        self as *const Self as usize as u64
    }
    async fn fetch(&self, request: DiscoveryRequest) -> Result<DiscoveryReply, DiscoveryError> {
        assert_eq!(request.url.to_utf8().unwrap(), "http://scope.test/v1/models");
        assert!(!request.headers.iter().any(|(name, _)| name.equals_ascii("Authorization")));
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(DiscoveryReply {
            status: 200,
            headers: Vec::new(),
            body: serde_json::to_vec(&json!({"data":[{"id":"found-model"}]})).unwrap(),
            json_override: None,
        })
    }
}
async fn startup_registry(root: &Path, discovery: bool, fetch: Arc<ScopeDiscovery>) -> ModelRegistry {
    let mut providers = json!({"q":{"api":"openai-completions","auth":"none",
        "baseUrl":"http://static.test/v1","models":[{"id":"q-model","name":"Static"}]}});
    if discovery {
        providers["p"] = json!({"api":"openai-completions","auth":"none",
            "baseUrl":"http://scope.test/v1",
            "discovery":{"type":"openai-models-list","injectV1":false}});
    }
    let config_path = root.join("models.json");
    std::fs::write(&config_path, json!({"providers":providers}).to_string()).unwrap();
    let mut disabled = OrderedProviderSet::default();
    for provider in startup_model_cache_provider_ids() {
        disabled.insert(provider);
    }
    for provider in ["ollama", "llama.cpp", "lm-studio"] {
        disabled.insert(provider.into());
    }
    let environment: Arc<dyn ProviderEnvironment> = Arc::new(|_: &str| None);
    let mut host = ModelRegistryHost::for_process(root.to_path_buf(), &root.join("models.sqlite")).unwrap();
    host.config_values = Arc::new(ConfigValueResolver::isolated());
    host.config_environment = Arc::new(|_: &str| None);
    host.factory = ProviderFactoryHost::new(environment.clone(), "scope-test", root.to_path_buf());
    host.credentials = Arc::new(EnvironmentRegistryCredentials::new(environment));
    host.context = CatalogContext::new(fetch).unwrap();
    // A fresh discovery receipt must follow the actual models file mtime.
    // Native deliberately bypasses even fresh cache rows older than config.
    let now = std::fs::metadata(root.join("models.json"))
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
        * 1000.0
        + 1000.0;
    host.clock = Arc::new(move || now);
    ModelRegistry::open(
        ModelsConfigFile::new(config_path).unwrap(),
        RegistryLoadOptions { disabled_providers: disabled, ..Default::default() },
        host,
    )
    .await
    .unwrap()
}

#[test]
#[cfg(windows)]
fn glob_scope_matches_fixed_native_suffix_unicode_path_class_and_order_oracle() {
    let mut models = [
        ("p", "model-a"),
        ("p", "model-b"),
        ("p", "model-1"),
        ("p", "nested/model-a"),
        ("p", ".hidden"),
        ("q", "model-a"),
        ("p", "model:max"),
        ("p", "model:auto"),
        ("p", "model:high"),
        ("p", "model"),
        ("p", "UPPER"),
        ("p", "中"),
        ("p", "😀"),
        ("p", "back\\slash"),
        ("p", "surrogate�"),
        ("p", "surrogate�"),
    ]
    .iter()
    .enumerate()
    .map(|(index, (provider, id))| model(provider, id, index))
    .collect::<Vec<_>>();
    // Bun converts ill-formed UTF-16 to U+FFFD at its UTF-8 glob boundary. Keep
    // distinct original catalogue identities while exercising that observation.
    let mut lone = models[14].spec().as_ref().clone();
    let mut id = "surrogate".encode_utf16().collect::<Vec<_>>();
    id.push(0xd800);
    lone.set("id", WireValue::String(WireString::from_units(id)));
    models[14] = HostModel::new(Arc::new(lone), HeaderSlot::Undefined).unwrap();
    let resolver = resolver(models, ModelMatchPreferences::default());
    let one_segment = &[0, 1, 2, 4, 6, 7, 8, 9, 10, 11, 12, 14, 15][..];
    let all = &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15][..];
    let all_p = &[0, 1, 2, 3, 4, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15][..];
    let cases: &[(&str, &[usize])] = &[
        ("*", &[0, 1, 2, 4, 5, 6, 7, 8, 9, 10, 11, 12, 14, 15]),
        ("p/*", one_segment),
        ("**", all),
        ("p/**", all_p),
        ("*/*", all),
        ("model-?", &[0, 1, 2, 5]),
        ("p/model-?", &[0, 1, 2]),
        ("p/model-[ab]", &[0, 1]),
        ("p/model-[!a]", &[1, 2]),
        ("p/model-[^a]", &[1, 2]),
        ("p/model-[a-b]", &[0, 1]),
        ("p/model-[0-9]", &[2]),
        ("p/[.]hidden", &[4]),
        ("?", &[11, 12]),
        ("p/?", &[11, 12]),
        ("p/*:high", one_segment),
        ("p/*:hi", one_segment),
        ("p/*:max", &[6]),
        ("p/*:ma", one_segment),
        ("p/*:auto", &[7]),
        ("p/*:AUTO", &[7]),
        ("p/*:bad", &[]),
        ("p/*:m", &[]),
        ("p/*:inherit", one_segment),
        ("p/*:in", one_segment),
        ("p/**/model-?", &[0, 1, 2, 3]),
        ("p/**/model-a", &[0, 3]),
        ("p/{model-a,model-b}*", &[0, 1]),
        ("!p/*", all),
        ("[", &[]),
        ("p/[", &[]),
        ("p/[z-a]", &[]),
        ("p/model-[]", &[]),
        ("p/model-[a", &[]),
        ("p/*\\*", &[]),
        ("p/back\\*", &[]),
        ("p/UP*", &[10]),
        ("*😀*", &[12]),
        ("p/surrogate?", &[14, 15]),
        ("p/surrogate[�]", &[14, 15]),
        ("p/**/**", all_p),
        ("p/**a", &[0]),
        ("p/*/**", &[3, 13]),
        ("p/[a-z]*", &[0, 1, 2, 6, 7, 8, 9, 10, 14, 15]),
        ("p/??", &[]),
        ("p/?*", one_segment),
        ("p/**/.*", &[4]),
        ("!!p/*", one_segment),
        ("!!!p/*", all),
        ("![", all),
        ("![z-a]", all),
        ("p/[中]*", &[11]),
        ("p/[😀]*", &[12]),
        ("p/[中-文]*", &[11]),
        ("p/[😀-😇]*", &[12]),
        ("p/{,中}*", one_segment),
        ("p/{model-a,{model-b,中}}*", &[0, 1, 11]),
        ("p/[a&&b]*", &[]),
        ("p/[a--c]*", &[]),
    ];
    for (pattern, expected) in cases {
        assert_eq!(indexes(&scopes(&resolver, &[*pattern])), *expected, "Native glob {pattern}");
    }
    for (patterns, expected) in [
        (vec![], all),
        (vec!["q/*", "p/*"], &[0, 1, 2, 4, 5, 6, 7, 8, 9, 10, 11, 12, 14, 15][..]),
        (vec!["model-*", "p/*"], &[0, 1, 2, 4, 5, 6, 7, 8, 9, 10, 11, 12, 14, 15][..]),
        (vec!["p/*:max", "p/model-?"], &[0, 1, 2, 6][..]),
        (vec!["[", "p/*"], one_segment),
        (vec!["nonexistent*"], &[][..]),
    ] {
        assert_eq!(indexes(&scopes(&resolver, &patterns)), expected, "Native ordered scope {patterns:?}");
    }
}

#[test]
#[cfg(windows)]
fn glob_classes_escape_metacharacters_and_preserve_native_invalid_range_members() {
    let resolver = resolver(
        ["a", "b", "c", "-", "]", "*", "&", "中", "😀", "\n", "a\nb"]
            .iter()
            .enumerate()
            .map(|(index, id)| model("p", id, index))
            .collect(),
        ModelMatchPreferences::default(),
    );
    // Bun primitive receipts: model-scope-oracle-extra.json and
    // model-scope-class-oracle.json. Whole scope uses unchanged Native snippets.
    for (pattern, expected) in [
        ("p/[a\\-c]", &[0, 2, 3][..]),
        ("p/[\\]]", &[4][..]),
        ("p/[\\*]", &[5][..]),
        ("p/[a&&b]", &[0, 1, 6][..]),
        ("p/[a--c]", &[2][..]),
        ("p/[中]", &[7][..]),
        ("p/[😀]", &[8][..]),
        ("p/?", &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9][..]),
        ("p/*", &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10][..]),
    ] {
        assert_eq!(indexes(&scopes(&resolver, &[pattern])), expected, "Native class scope {pattern}");
    }
}

#[test]
fn direct_scope_retains_default_preferences_and_existing_literal_objects() {
    let first = model("custom-a", "same", 0);
    let recent = model("custom-b", "same", 1);
    let router = model("openrouter", "qwen/qwen3-coder:exacto", 2);
    let codex = model("openai-codex", "gpt-5.5", 3);
    let openai = model("openai", "gpt-5.5", 4);
    let resolver = resolver(
        vec![first.clone(), recent, router.clone(), codex, openai.clone()],
        ModelMatchPreferences { usage_order: vec!["custom-b/same".into()], ..Default::default() },
    );
    let direct = scopes(&resolver, &["same"]);
    assert!(Arc::ptr_eq(&direct[0], &first), "ordinary patterns do not inherit settings MRU");
    let settings = |role: &str| (role == "fable").then(|| "same".to_owned());
    assert_eq!(indexes(&filter(&resolver, &["@fable".into()], &settings).unwrap()), [1]);
    assert_eq!(indexes(&filter(&resolver, &["pi/fable".into()], &settings).unwrap()), [1]);
    for pattern in ["openrouter/qwen/qwen3-coder:exacto", "qwen/qwen3-coder:exacto", "openrouter/qwen-qwen3-coder"] {
        let selected = scopes(&resolver, &[pattern]);
        assert_eq!(indexes(&selected), [2], "exact/bare/provider fuzzy {pattern}");
        assert!(Arc::ptr_eq(&selected[0], &router));
    }
    let selected = scopes(&resolver, &["openai/gpt-5.5:high"]);
    assert!(Arc::ptr_eq(&selected[0], &openai), "explicit provider does not coalesce to Codex");
    assert!(scopes(&resolver, &["no-such-model"]).is_empty());
    assert_eq!(indexes(&scopes(&resolver, &["openai/gpt-5.5", "same", "same"])), [0, 4]);
}

#[test]
fn synthetic_profiles_and_role_routes_keep_native_key_dedupe_and_donors() {
    let donor = create_live_config_headers(
        &[Some(HeaderSource::Config(HeaderConfigRecord::default()))],
        HeaderResolutionOptions::default(),
    )
    .unwrap();
    let bedrock = HostModel::new(
        Arc::new(VariantSpec::from_json(&json!({"provider":"amazon-bedrock","id":"default","name":"default"}))),
        HeaderSlot::Source(HeaderSource::Live(donor.clone())),
    )
    .unwrap();
    let router = model("openrouter", "vendor/router", 1);
    let resolver = resolver(vec![bedrock, router.clone()], ModelMatchPreferences::default());
    let first = "arn:aws:bedrock:us-east-2:1234567890:application-inference-profile/company-opus-48";
    let second = "arn:aws:bedrock:us-east-2:1234567890:application-inference-profile/company-sonnet-48";
    let scoped = scopes(&resolver, &[second, first, second, "openrouter/vendor/router"]);
    assert!(Arc::ptr_eq(&scoped[0], &router), "catalogue entries precede synthetic patterns");
    assert_eq!(ids(&scoped), ["vendor/router", second, first]);
    for profile in &scoped[1..] {
        donor.set("x-retained-profile", "scope-test");
        match profile.headers() {
            HeaderSlot::Source(HeaderSource::Live(headers)) => assert!(headers.delete("x-retained-profile")),
            _ => panic!("synthetic profile lost its header donor"),
        }
    }

    // Ordinary filter uses the direct parser; only roles have the separate
    // routing fallback. Native inclusion dedupes a routed same-key copy back
    // to its original available catalogue object.
    assert!(scopes(&resolver, &["openrouter/vendor/router@cerebras"]).is_empty());
    let settings = |role: &str| (role == "routed").then(|| "openrouter/vendor/router@cerebras:high".to_owned());
    let scoped = filter(&resolver, &["@routed".into()], &settings).unwrap();
    assert_eq!(scoped.len(), 1);
    assert!(Arc::ptr_eq(&scoped[0], &router));
    assert!(scoped[0].spec().get("compatConfig").is_none());
}

#[test]
fn max_auto_literal_precedence_and_invalid_suffix_scope_are_source_backed() {
    let resolver = resolver(vec![model("p", "unit", 0)], ModelMatchPreferences::default());
    assert_eq!(indexes(&scopes(&resolver, &["p/*:max"])), [0]);
    assert_eq!(indexes(&scopes(&resolver, &["p/*:auto"])), [0]);
    assert!(scopes(&resolver, &["p/*:bad"]).is_empty());
    // Invalid non-glob suffixes use the existing parser's warning/fallback
    // contract; invalid glob suffixes remain part of the glob.
    assert_eq!(indexes(&scopes(&resolver, &["p/unit:bad"])), [0]);
    assert_eq!(indexes(&scopes(&resolver, &["p/unit:high"])), [0]);
}

#[test]
fn cycling_scope_retains_pattern_catalog_order_first_override_and_baked_effort_clamp() {
    let p_a = reasoning_model("p", "a", 0, &["low", "high"]);
    let p_b = reasoning_model("p", "b", 1, &["medium", "high"]);
    let q_a = model("q", "a", 2);
    let resolver = resolver(vec![p_a.clone(), p_b.clone(), q_a.clone()], ModelMatchPreferences::default());
    let no_roles = |_role: &str| None;
    let scoped = cycling_scopes(&resolver, &["q/a:off", "p/*:xhigh", "p/a:low", "q/a:high"], &no_roles);
    assert_eq!(
        cycling_observations(&scoped),
        [
            ("q/a".into(), Some(ThinkingLevel::Off), true),
            ("p/a".into(), Some(ThinkingLevel::High), true),
            ("p/b".into(), Some(ThinkingLevel::High), true),
        ]
    );
    for (entry, donor) in scoped.iter().zip([&q_a, &p_a, &p_b]) {
        assert!(Arc::ptr_eq(&entry.model, donor), "scope must keep each actual model donor");
    }
    assert_eq!(
        cycling_observations(&cycling_scopes(&resolver, &["p/a", "p/a:high"], &no_roles)),
        [("p/a".into(), None, false)],
        "the first absence also wins deduplication"
    );
    assert_eq!(
        cycling_observations(&cycling_scopes(&resolver, &["p/b:minimal", "q/a:high", "p/a:inherit"], &no_roles)),
        [
            ("p/b".into(), Some(ThinkingLevel::Medium), true),
            ("q/a".into(), Some(ThinkingLevel::High), true),
            ("p/a".into(), Some(ThinkingLevel::Inherit), true),
        ],
        "Native floor and clamp-result fallback retain Off/Inherit or an uncontrolled selector"
    );
    assert!(cycling_scopes(&resolver, &[], &no_roles).is_empty());
    assert_eq!(indexes(&scopes(&resolver, &[])), [0, 1, 2], "existing enabled filter's empty behavior is unchanged");
    // Native clamp reads baked efforts regardless of the provider's wire mode.
    // Exercise these within the cycling family rather than through OpenAI's
    // narrower transport-policy parser.
    for mode in ["effort", "budget", "google-level", "anthropic-adaptive", "anthropic-budget-effort"] {
        let donor = HostModel::new(
            Arc::new(VariantSpec::from_json(&json!({"provider":"mode","id":"unit","name":"Mode unit",
                "reasoning":true,"thinking":{"mode":mode,"efforts":["medium","high"]}}))),
            HeaderSlot::Undefined,
        )
        .unwrap();
        let mode_resolver = ModelRoleResolver::new(
            vec![donor.clone()],
            Arc::new(Mutex::new(CollapseRuntime::new().unwrap())),
            ModelMatchPreferences::default(),
        )
        .unwrap();
        let scoped = cycling_scopes(&mode_resolver, &["mode/unit:xhigh"], &no_roles);
        assert_eq!(
            cycling_observations(&scoped),
            [("mode/unit".into(), Some(ThinkingLevel::High), true)],
            "mode {mode}"
        );
        assert!(Arc::ptr_eq(&scoped[0].model, &donor));
    }
}

#[test]
fn cycling_scope_uses_current_preferences_one_role_model_and_session_owned_auto() {
    let first = model("custom-a", "same", 0);
    let recent = model("custom-b", "same", 1);
    let fallback = model("p", "fallback", 2);
    let router = model("openrouter", "vendor/router", 3);
    let resolver = resolver(
        vec![first.clone(), recent.clone(), fallback, router.clone()],
        ModelMatchPreferences { usage_order: vec!["custom-b/same".into()], ..Default::default() },
    );
    let settings = |role: &str| match role {
        "preferred" => Some("same:high".into()),
        "chain" => Some("missing/nope,custom-a/same:auto,p/fallback:off".into()),
        "routed" => Some("openrouter/vendor/router@cerebras:high".into()),
        _ => None,
    };
    let direct = cycling_scopes(&resolver, &["same:high"], &settings);
    assert!(Arc::ptr_eq(&direct[0].model, &recent), "cycling direct patterns inherit current MRU preferences");
    assert!(Arc::ptr_eq(&scopes(&resolver, &["same:high"])[0], &first), "enabled filter retains default preferences");
    let scoped = cycling_scopes(&resolver, &["@preferred", "pi/chain", "custom-a/same:low", "@routed"], &settings);
    assert_eq!(
        cycling_observations(&scoped),
        [
            ("custom-b/same".into(), Some(ThinkingLevel::High), true),
            ("custom-a/same".into(), None, false),
            ("openrouter/vendor/router".into(), Some(ThinkingLevel::High), true),
        ],
        "a role contributes one model; Auto is dropped before first-key deduplication"
    );
    assert!(Arc::ptr_eq(&scoped[0].model, &recent));
    assert!(Arc::ptr_eq(&scoped[1].model, &first));
    assert_eq!(
        ara_cli::model_roles::format_model_string_with_routing(&scoped[2].model).to_utf8().unwrap(),
        "openrouter/vendor/router@cerebras",
        "cycling retains the first routed model copy"
    );
    assert!(scoped[2].model.headers().as_source().is_none());
    assert_eq!(
        cycling_observations(&cycling_scopes(&resolver, &["custom-b/same:auto", "custom-b/same:high"], &settings)),
        [("custom-b/same".into(), None, false)]
    );
}

#[test]
fn cycling_globs_preserve_frozen_native_suffix_flags_literal_max_and_auto_precedence() {
    let no_roles = |_role: &str| None;
    let unit = reasoning_model("p", "unit", 0, &["low", "high"]);
    let single = resolver(vec![unit.clone()], ModelMatchPreferences::default());
    // Frozen Bun resolveGlobScopePattern receipts have high/hi -> high,true;
    // ma -> max,true and literal max/auto -> undefined,false. The missing
    // literal max/auto cases follow fixed helper source at lines 182-192.
    // Scoped admission then clamps those raw efforts against the baked ladder.
    for (pattern, level, explicit) in [
        ("p/*:high", Some(ThinkingLevel::High), true),
        ("p/*:hi", Some(ThinkingLevel::High), true),
        ("p/*:max", Some(ThinkingLevel::High), true),
        ("p/*:ma", Some(ThinkingLevel::High), true),
        ("p/*:auto", None, false),
        ("p/*:inherit", Some(ThinkingLevel::Inherit), true),
    ] {
        let scoped = cycling_scopes(&single, &[pattern], &no_roles);
        assert_eq!(cycling_observations(&scoped), [("p/unit".into(), level, explicit)], "Native suffix {pattern}");
        assert!(Arc::ptr_eq(&scoped[0].model, &unit));
    }
    let literal = resolver(
        vec![model("p", "unit", 0), model("p", "unit:max", 1), model("p", "unit:auto", 2)],
        ModelMatchPreferences::default(),
    );
    for (pattern, expected) in [
        ("p/*:max", "p/unit:max"),
        ("p/*:auto", "p/unit:auto"),
        ("p/*:AUTO", "p/unit:auto"),
        ("p/unit:max", "p/unit:max"),
    ] {
        assert_eq!(
            cycling_observations(&cycling_scopes(&literal, &[pattern], &no_roles)),
            [(expected.into(), None, false)],
            "literal tier wins {pattern}"
        );
    }
    assert!(cycling_scopes(&literal, &["p/*:bad"], &no_roles).is_empty());
    assert_eq!(
        cycling_observations(&cycling_scopes(&literal, &["p/*:auto", "p/*:high"], &no_roles)),
        [
            ("p/unit:auto".into(), None, false),
            ("p/unit".into(), Some(ThinkingLevel::High), true),
            ("p/unit:max".into(), Some(ThinkingLevel::High), true),
        ],
        "literal match order precedes later broad-glob catalog order"
    );
}

#[tokio::test]
async fn startup_scope_discovers_only_completely_empty_known_scopes_and_keeps_deferred_empty_results() {
    let no_roles = |_role: &str| None;
    let preferences = ModelMatchPreferences::default();
    for (discovery, patterns, expected, expected_calls) in [
        (true, vec!["p/found-model"], vec!["p/found-model"], 1),
        (true, vec!["q/q-model", "p/found-model"], vec!["q/q-model"], 0),
        (true, vec![], vec![], 0),
        (false, vec!["extprov/not-registered"], vec![], 0),
        (true, vec!["extprov/not-registered"], vec![], 1),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let fetch = Arc::new(ScopeDiscovery::default());
        let registry = startup_registry(temp.path(), discovery, fetch.clone()).await;
        let patterns = patterns.into_iter().map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(fetch.calls.load(Ordering::SeqCst), 0, "opening is offline");
        assert_eq!(registry.get_discoverable_providers().len(), usize::from(discovery));
        let scoped = resolve_scoped_models(&registry, &patterns, &preferences, &no_roles).await.unwrap();
        assert_eq!(cycling_observations(&scoped).into_iter().map(|entry| entry.0).collect::<Vec<_>>(), expected);
        assert_eq!(fetch.calls.load(Ordering::SeqCst), expected_calls, "patterns {patterns:?}");
        let current = resolve_current_scoped_models(&registry, &patterns, &preferences, &no_roles).unwrap();
        assert_eq!(cycling_observations(&current), cycling_observations(&scoped));
        assert_eq!(fetch.calls.load(Ordering::SeqCst), expected_calls, "current-only rebuild never fetches");
        if expected_calls == 1 {
            // The refresh owner reuses the fresh cache on another unresolved
            // scope; this helper must not bypass online-if-uncached semantics.
            assert!(
                resolve_scoped_models(&registry, &["extprov/still-absent".into()], &preferences, &no_roles)
                    .await
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(fetch.calls.load(Ordering::SeqCst), 1, "fresh-cache discovery does not refetch");
        }
    }
}

#[test]
fn session_scope_mapping_keeps_native_defaults_donors_and_identity_only_set_comparison() {
    let a = reasoning_model("p", "a", 0, &["low"]);
    let b = model("q", "b", 1);
    let scoped = [
        ScopedModel { model: a.clone(), thinking_level: None, explicit_thinking_level: false },
        ScopedModel { model: b.clone(), thinking_level: Some(ThinkingLevel::Low), explicit_thinking_level: false },
        ScopedModel { model: a.clone(), thinking_level: Some(ThinkingLevel::Off), explicit_thinking_level: true },
        ScopedModel { model: b.clone(), thinking_level: Some(ThinkingLevel::Inherit), explicit_thinking_level: true },
        ScopedModel { model: a.clone(), thinking_level: None, explicit_thinking_level: true },
    ];
    for (default, expected) in [
        (
            Some(ConfiguredThinkingLevel::Concrete(ThinkingLevel::High)),
            [
                Some(ThinkingLevel::High),
                Some(ThinkingLevel::High),
                Some(ThinkingLevel::Off),
                Some(ThinkingLevel::Inherit),
                Some(ThinkingLevel::High),
            ],
        ),
        (
            Some(ConfiguredThinkingLevel::Auto),
            [None, None, Some(ThinkingLevel::Off), Some(ThinkingLevel::Inherit), None],
        ),
        (None, [None, None, Some(ThinkingLevel::Off), Some(ThinkingLevel::Inherit), None]),
    ] {
        let mapped = to_session_scoped_models(&scoped, default);
        assert_eq!(
            mapped.iter().map(|entry| entry.thinking_level).collect::<Vec<_>>(),
            expected,
            "implicit entries use the Session default, without model clamping"
        );
        for (entry, source) in mapped.iter().zip(&scoped) {
            assert!(Arc::ptr_eq(&entry.model, &source.model));
        }
    }
    let pair = to_session_scoped_models(&scoped[..2], Some(ConfiguredThinkingLevel::Auto));
    let revised = [
        SessionScopedModel { model: model("q", "b", 99), thinking_level: Some(ThinkingLevel::Max) },
        SessionScopedModel {
            model: reasoning_model("p", "a", 100, &["max"]),
            thinking_level: Some(ThinkingLevel::High),
        },
    ];
    assert!(
        same_scoped_model_set(&pair, &revised),
        "order, thinking, metadata and donor replacement do not alter keys"
    );
    assert!(!same_scoped_model_set(&pair, &revised[..1]));
    let changed = [revised[0].clone(), SessionScopedModel { model: model("p", "other", 100), thinking_level: None }];
    assert!(!same_scoped_model_set(&pair, &changed));
    assert!(same_scoped_model_set(&[], &[]));
    assert!(to_session_scoped_models(&[], Some(ConfiguredThinkingLevel::Concrete(ThinkingLevel::High))).is_empty());
}
