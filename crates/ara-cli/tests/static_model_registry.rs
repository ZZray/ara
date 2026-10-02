//! Grouped native static-registry scenarios from fixed OMP 596f2da. Snapshot
//! composition is not acceptance of discovery, live auth or selector routing.

use ara_cli::{
    model_collapse::VariantSpec,
    model_config_values::{
        CommandConfigCache, ConfigCommandExecutor, ConfigCommandFailure, ConfigValueContext, ConfigValueResolver,
        HeaderConfigRecord, HeaderSource, SystemConfigValueClock,
    },
    model_identity_wire::{bundled_models, text},
    model_patch::{HeaderSlot, HostModel, HostModelRef, ModelPatch, OrderedProviderSet},
    models_config::ModelsConfig,
    static_model_registry::{StaticModelRegistry, StaticRegistryInputs},
};
use ara_rpc::{WireString, WireValue};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

fn header_slot(value: &str) -> HeaderSlot {
    HeaderSlot::Source(HeaderSource::Config(HeaderConfigRecord::from_pairs(vec![("x-owner".into(), value.into())])))
}

fn model(provider: &str, id: &str, headers: HeaderSlot) -> HostModelRef {
    // Explicit complete snapshot rows: no accidental builder normalization.
    HostModel::new(
        Arc::new(VariantSpec::from_json(&json!({"provider":provider,"id":id,"name":id,
        "api":"openai-completions","baseUrl":"https://bundle.test/v1","reasoning":false,"input":["text"],
        "cost":{"input":1,"output":2,"cacheRead":3,"cacheWrite":4},"contextWindow":128000,"maxTokens":8192,
        "supportsTools":false,"compat":{"resolved":"bundle"},"compatConfig":{"extraBody":{"authored":"bundle"}}}))),
        headers,
    )
    .unwrap()
}

fn changed(model: &HostModelRef, edit: impl FnOnce(&mut VariantSpec)) -> HostModelRef {
    let mut fields = model.spec().as_ref().clone();
    edit(&mut fields);
    model.with_spec(Arc::new(fields)).unwrap()
}

fn config(value: Value) -> ModelsConfig {
    ModelsConfig::validate(value).unwrap()
}
fn wire(model: &HostModelRef, key: &str) -> Option<WireValue> {
    model.spec().get(key).cloned()
}
fn s(value: &str) -> WireValue {
    WireValue::String(value.into())
}
fn number(value: f64) -> Option<WireValue> {
    Some(WireValue::Number(value))
}
fn providers(values: &[&str]) -> OrderedProviderSet {
    let mut out = OrderedProviderSet::default();
    for value in values {
        out.insert((*value).into());
    }
    out
}

struct Executor(AtomicUsize);
impl ConfigCommandExecutor for Executor {
    fn directory_is_enterable(&self, _: &Path) -> bool {
        true
    }
    fn execute(&self, command: &str, _: &Path) -> Result<String, ConfigCommandFailure> {
        assert_eq!(command, "synthetic-header");
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok("resolved-header".into())
    }
}
fn snapshot(model: &HostModelRef, resolver: &ConfigValueResolver) -> Vec<(String, String)> {
    let environment = |_: &str| None;
    let context = ConfigValueContext { project_dir: Path::new("."), environment: &environment };
    match model.headers() {
        HeaderSlot::Source(HeaderSource::Live(headers)) => {
            headers.snapshot(resolver, &context).map(|value| value.into_pairs()).unwrap_or_default()
        }
        _ => panic!("expected the composed opaque Live source"),
    }
}

#[test]
fn bundled_rows_stay_verbatim_shared_and_provider_lookup_is_lazy_and_interned() {
    let first = model("p", "a", HeaderSlot::Absent);
    let second = model("p", "b", HeaderSlot::Absent);
    let untouched = model("other", "c", header_slot("literal"));
    let registry = StaticModelRegistry::from_inputs(
        None,
        StaticRegistryInputs {
            bundled_models: vec![first.clone(), second.clone(), untouched.clone()],
            ..Default::default()
        },
    )
    .unwrap();
    let lookup = registry.models_for_provider_lookup(&" P ".into()).unwrap();
    assert_eq!(lookup.len(), 2);
    assert!(Arc::ptr_eq(&lookup[0], &first));
    assert!(Arc::ptr_eq(&lookup[1], &second));
    assert_eq!(wire(&lookup[0], "compat"), Some(VariantSpec::from_json(&json!({"resolved":"bundle"})).value));
    assert!(registry.find_exact(&"P".into(), &"a".into()).unwrap().is_none());
    let all = registry.get_all().unwrap();
    assert!(Arc::ptr_eq(&all[0], &lookup[0]));
    assert!(Arc::ptr_eq(&all[2], &untouched));
    assert_eq!(
        registry.models_for_provider_lookup(&"p".into()).unwrap().len(),
        3,
        "full snapshot lookup keeps the native whole-catalog fast path"
    );
    let callbacks = AtomicUsize::new(0);
    let available = registry
        .get_available_for_providers(&providers(&["p"]), &|provider| {
            callbacks.fetch_add(1, Ordering::SeqCst);
            assert!(registry.find_exact(provider, &"a".into()).unwrap().is_some());
            true
        })
        .unwrap();
    assert_eq!(available.len(), 2);
    assert_eq!(callbacks.load(Ordering::SeqCst), 1, "full snapshot auth callback can re-enter lookup and is memoized");
    let again = registry.get_all().unwrap();
    assert!(all.iter().zip(&again).all(|(a, b)| Arc::ptr_eq(a, b)));
    // Real fixed bundle, shared across distinct registry instances; source
    // models.ts only adds identity, it does not re-run the policy builder.
    let raw = bundled_models()
        .iter()
        .find(|row| {
            text(row, "provider").is_some_and(|p| p.equals_ascii("xiaomi"))
                && text(row, "id").is_some_and(|id| id.equals_ascii("mimo-v2.5"))
        })
        .unwrap();
    let a = StaticModelRegistry::from_config(None).unwrap();
    let b = StaticModelRegistry::from_config(None).unwrap();
    let a = a.find_exact(&"xiaomi".into(), &"mimo-v2.5".into()).unwrap().unwrap();
    let b = b.find_exact(&"xiaomi".into(), &"mimo-v2.5".into()).unwrap().unwrap();
    assert!(Arc::ptr_eq(&a, &b));
    assert_eq!(a.spec().get("compat"), raw.get("compat"));
    assert_eq!(a.spec().get("cost"), raw.get("cost"));
}

#[test]
fn configured_overlays_replace_same_id_transport_and_lookup_never_resolves_raw_credentials() {
    let base = model("custom", "same", header_slot("bundled"));
    let custom = config(json!({"providers":{"custom":{"baseUrl":"https://provider.test/v1","api":"openai-responses",
        "apiKey":"!synthetic-key-must-remain-raw","auth":"none","headers":{"x-provider":"!synthetic-header"},
        "disableStrictTools":true,"compat":{"extraBody":{"provider":true}},"remoteCompaction":{"shared":"provider","kept":1},
        "guardrailIdentifier":"guard","guardrailVersion":"2","guardrailTrace":"enabled","requestMetadata":{"source":"test"},
        "models":[{"id":"same","name":"Configured","baseUrl":"https://model.test/v1","headers":{"x-model":"model"},
            "compat":{"extraBody":{"model":true}},"remoteCompaction":{"shared":"model"}},
            {"id":"new-model"}],"modelOverrides":{"same":{"contextWindow":222222,"headers":{"x-override":"override"}}}}}}));
    let registry = StaticModelRegistry::from_inputs(
        Some(&custom),
        StaticRegistryInputs { bundled_models: vec![base], ..Default::default() },
    )
    .unwrap();
    assert!(registry.is_keyless_provider(&"custom".into()));
    assert_eq!(registry.configured_api_key(&"custom".into()), Some("!synthetic-key-must-remain-raw"));
    assert!(matches!(
        registry.configured_provider_headers(&"custom".into()),
        Some(HeaderSlot::Source(HeaderSource::Config(_)))
    ));
    let executor = Arc::new(Executor(AtomicUsize::new(0)));
    let resolver = ConfigValueResolver::with_ports(
        Arc::new(CommandConfigCache::default()),
        Arc::new(SystemConfigValueClock),
        executor.clone(),
    );
    let same = registry.find_exact(&"custom".into(), &"same".into()).unwrap().unwrap();
    assert_eq!(executor.0.load(Ordering::SeqCst), 0);
    assert_eq!(wire(&same, "baseUrl"), Some(s("https://model.test/v1")));
    assert_eq!(wire(&same, "name"), Some(s("Configured")));
    assert_eq!(wire(&same, "api"), Some(s("openai-responses")));
    // Bedrock provider rebuild follows the explicit model override. The native
    // unknown model has no catalog correction, so this authored value survives.
    assert_eq!(wire(&same, "contextWindow"), number(222222.0));
    for (key, value) in [("guardrailIdentifier", "guard"), ("guardrailVersion", "2"), ("guardrailTrace", "enabled")] {
        assert_eq!(wire(&same, key), Some(s(value)));
    }
    let facts: Value = serde_json::from_str(&same.spec().to_wire_json().stringify()).unwrap();
    assert_eq!(facts["compatConfig"]["extraBody"], json!({"provider":true,"model":true}));
    assert_eq!(facts["compatConfig"]["disableStrictTools"], true);
    assert_eq!(facts["remoteCompaction"], json!({"shared":"model","kept":1}));
    assert!(same.spec().get("headers").is_none() && same.spec().get("apiKey").is_none());
    let headers = snapshot(&same, &resolver);
    assert_eq!(
        headers,
        [
            ("x-provider".into(), "resolved-header".into()),
            ("x-model".into(), "model".into()),
            ("x-override".into(), "override".into())
        ]
    );
    assert_eq!(executor.0.load(Ordering::SeqCst), 1);
    let standalone = registry.find_exact(&"custom".into(), &"new-model".into()).unwrap().unwrap();
    assert_eq!(wire(&standalone, "contextWindow"), number(128000.0));
    assert_eq!(wire(&standalone, "maxTokens"), number(16384.0));
    assert_eq!(wire(&standalone, "reasoning"), Some(WireValue::Bool(false)));
    // Catalog existence and configured key do not mark a non-keyless provider
    // authenticated. The availability observation remains Host owned.
    let raw = config(json!({"providers":{"p":{"apiKey":"!raw-not-run","headers":{"x":"!raw-not-run"}}}}));
    let registry = StaticModelRegistry::from_inputs(
        Some(&raw),
        StaticRegistryInputs { bundled_models: vec![model("p", "id", HeaderSlot::Absent)], ..Default::default() },
    )
    .unwrap();
    assert!(registry.get_available_for_providers(&providers(&["p"]), &|_| false).unwrap().is_empty());
}

#[test]
fn snapshot_composition_keeps_native_merge_order_nullish_fields_metrics_position_and_interning() {
    let a = changed(&model("p", "a", header_slot("base")), |fields| {
        fields.set("contextWindow", WireValue::Number(111.0));
        fields.set("maxTokens", WireValue::Number(222.0));
        fields.set("omitMaxOutputTokens", WireValue::Bool(true));
    });
    let cache_a = changed(&model("p", "a", header_slot("cached")), |fields| {
        fields.set("contextWindow", WireValue::Null);
        fields.set_undefined("maxTokens");
        fields.set_undefined("supportsTools");
    });
    let discovered_a = changed(&model("p", "a", header_slot("discovered")), |fields| {
        fields.set("contextWindow", WireValue::Number(888.0));
        fields.set("maxTokens", WireValue::Number(500.0));
        fields.set("omitMaxOutputTokens", WireValue::Bool(false));
        fields.set("supportsTools", WireValue::Bool(true));
    });
    let overlay = ModelPatch::new(
        VariantSpec::from_json(&json!({"provider":"p","id":"a","api":"openai-responses",
        "baseUrl":"https://runtime.test/v1","name":"runtime-overlay"})),
        HeaderSlot::Absent,
    )
    .unwrap();
    let config = config(
        json!({"providers":{"p":{"baseUrl":"https://configured.test/v1","api":"openai-completions","auth":"none",
        "models":[{"id":"a","name":"configured-overlay"}],"modelOverrides":{"a":{"contextWindow":777}}},
        "s":{"baseUrl":"https://standalone.test/v1","api":"openai-completions","auth":"none","models":[{"id":"only"}]}}}),
    );
    let registry = StaticModelRegistry::from_inputs(
        Some(&config),
        StaticRegistryInputs {
            bundled_models: vec![
                a,
                model("p", "b", HeaderSlot::Absent),
                model("q", "bundled", HeaderSlot::Absent),
                model("r", "bundled", HeaderSlot::Absent),
            ],
            cached_standard_models: vec![
                cache_a,
                model("p", "c", HeaderSlot::Absent),
                model("r", "cached", HeaderSlot::Absent),
                model("q", "cached", HeaderSlot::Absent),
            ],
            cached_discoverable_models: vec![discovered_a],
            runtime_discovered_models: vec![model("r", "runtime", HeaderSlot::Absent)],
            cached_authoritative_providers: providers(&["r"]),
            runtime_authoritative_providers: providers(&["q"]),
            runtime_model_overlays: vec![overlay],
            metrics_models: vec![Arc::new(VariantSpec::from_json(&json!({"id":"a","int":99,"tps":123})))],
            ..Default::default()
        },
    )
    .unwrap();
    let lazy = registry.find_exact(&"p".into(), &"a".into()).unwrap().unwrap();
    assert_eq!(wire(&lazy, "name"), Some(s("runtime-overlay")));
    assert_eq!(wire(&lazy, "api"), Some(s("openai-responses")));
    assert_eq!(wire(&lazy, "baseUrl"), Some(s("https://runtime.test/v1")));
    assert_eq!(wire(&lazy, "contextWindow"), number(777.0));
    assert_eq!(wire(&lazy, "maxTokens"), number(500.0));
    assert_eq!(wire(&lazy, "omitMaxOutputTokens"), Some(WireValue::Bool(false)));
    assert_eq!(wire(&lazy, "supportsTools"), Some(WireValue::Bool(true)));
    assert_eq!(wire(&lazy, "int"), number(99.0));
    assert_eq!(wire(&lazy, "tps"), number(123.0));
    assert!(matches!(lazy.headers(), HeaderSlot::Undefined));
    let all = registry.get_all().unwrap();
    let ids: Vec<_> =
        all.iter().map(|model| (text(model.spec(), "provider").unwrap(), text(model.spec(), "id").unwrap())).collect();
    assert_eq!(
        ids,
        [("p", "a"), ("p", "b"), ("p", "c"), ("r", "cached"), ("r", "runtime"), ("s", "only")]
            .map(|(provider, id)| (WireString::from(provider), WireString::from(id)))
    );
    assert!(Arc::ptr_eq(&all[0], &lazy));
    let unbuilt = StaticModelRegistry::from_inputs(
        None,
        StaticRegistryInputs {
            bundled_models: vec![model("p", "a", HeaderSlot::Absent)],
            cached_standard_models: vec![changed(&model("p", "a", HeaderSlot::Absent), |fields| {
                fields.set("contextWindow", WireValue::Null);
                fields.set_undefined("maxTokens");
                fields.set_undefined("supportsTools");
            })],
            ..Default::default()
        },
    )
    .unwrap();
    let fallback = unbuilt.find_exact(&"p".into(), &"a".into()).unwrap().unwrap();
    assert_eq!(wire(&fallback, "contextWindow"), number(128000.0));
    assert_eq!(wire(&fallback, "maxTokens"), number(8192.0));
    assert_eq!(wire(&fallback, "supportsTools"), Some(WireValue::Bool(false)));
}

#[test]
fn hardcoded_policies_availability_memoization_and_provider_isolation_match_static_contract() {
    let tiered = |provider: &str, id: &str| {
        changed(&model(provider, id, HeaderSlot::Absent), |fields| {
            fields.set("contextWindow", WireValue::Number(800000.0));
            fields.set(
                "cost",
                VariantSpec::from_json(&json!({"input":1,"output":2,"cacheRead":3,"cacheWrite":4,
            "longContext":{"inputThreshold":272000,"input":2,"output":4,"cacheRead":6,"cacheWrite":8}}))
                .value,
            );
        })
    };
    let config = config(
        json!({"providers":{"openai":{"modelOverrides":{"gpt-5.4":{"contextWindow":256000},"gpt-5.6":{"contextWindow":500000}}},
        "local":{"auth":"none"},"broken":{"guardrailIdentifier":"guard"}}}),
    );
    let broken = changed(&model("broken", "bad", HeaderSlot::Absent), |fields| fields.set("name", WireValue::Null));
    let registry = StaticModelRegistry::from_inputs(
        Some(&config),
        StaticRegistryInputs {
            bundled_models: vec![
                tiered("openai", "gpt-5.4"),
                tiered("openai", "gpt-5.6"),
                tiered("xai-oauth", "gpt-5.6"),
                model("ollama-cloud", "unit", HeaderSlot::Absent),
                model("local", "a", HeaderSlot::Absent),
                model("local", "b", HeaderSlot::Absent),
                broken,
            ],
            extended_context: false,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        wire(&registry.find_exact(&"openai".into(), &"gpt-5.4".into()).unwrap().unwrap(), "contextWindow"),
        number(256000.0)
    );
    assert_eq!(
        wire(&registry.find_exact(&"openai".into(), &"gpt-5.6".into()).unwrap().unwrap(), "contextWindow"),
        number(500000.0)
    );
    assert_eq!(
        wire(&registry.find_exact(&"xai-oauth".into(), &"gpt-5.6".into()).unwrap().unwrap(), "contextWindow"),
        number(800000.0)
    );
    assert_eq!(
        wire(&registry.find_exact(&"ollama-cloud".into(), &"unit".into()).unwrap().unwrap(), "omitMaxOutputTokens"),
        Some(WireValue::Bool(true))
    );
    let checks = AtomicUsize::new(0);
    let available = registry
        .get_available_for_providers(&providers(&[" LOCAL ", "xai-oauth"]), &|provider| {
            assert!(provider.equals_ascii("xai-oauth"));
            checks.fetch_add(1, Ordering::SeqCst);
            true
        })
        .unwrap();
    assert_eq!(checks.load(Ordering::SeqCst), 1);
    assert_eq!(available.len(), 3);
    let reentered = registry
        .get_available_for_providers(&providers(&["xai-oauth"]), &|provider| {
            assert!(registry.find_exact(provider, &"gpt-5.6".into()).unwrap().is_some());
            true
        })
        .unwrap();
    assert_eq!(reentered.len(), 1, "lazy availability callback runs before taking the composition mutex");
    assert!(registry.get_all().is_err(), "unrelated malformed provider is touched only by a full composition");
    assert!(registry.find_exact(&"local".into(), &"a".into()).unwrap().is_some());
    let disabled = StaticModelRegistry::from_inputs(
        None,
        StaticRegistryInputs {
            bundled_models: vec![model("p", "a", HeaderSlot::Absent), model("p", "b", HeaderSlot::Absent)],
            disabled_providers: providers(&["p"]),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        disabled
            .get_available_for_providers(&providers(&["p"]), &|_| panic!("disabled provider must not query auth"))
            .unwrap()
            .is_empty()
    );
    assert_eq!(disabled.get_all().unwrap().len(), 2, "disabled providers remain present in the full catalog");
}

#[test]
fn collapse_uses_actual_family_first_member_sidecar_then_rebinds_retired_alias_overrides() {
    let config = config(
        json!({"providers":{"newapi":{"baseUrl":"https://proxy.test/v1","api":"openai-completions","auth":"none",
        "models":[{"id":"unit-thinking","headers":{"x-donor":"thinking"}},
            {"id":"unit","headers":{"x-donor":"bare"}},
            {"id":"observer","headers":{"x-donor":"observer"},"compactionModel":"unit-thinking"}],
        "modelOverrides":{"unit-thinking":{"contextWindow":222222}}}}}),
    );
    let registry = StaticModelRegistry::from_inputs(Some(&config), StaticRegistryInputs::default()).unwrap();
    let bare = registry.find_exact(&"newapi".into(), &"unit".into()).unwrap().unwrap();
    assert_eq!(wire(&bare, "contextWindow"), number(222222.0));
    assert_eq!(wire(&bare, "reasoning"), Some(WireValue::Bool(true)));
    assert!(registry.find_exact(&"newapi".into(), &"unit-thinking".into()).unwrap().is_none());
    let alias = registry.find_alias_exact(&"newapi".into(), &"unit-thinking".into()).unwrap().unwrap();
    assert!(Arc::ptr_eq(&bare, &alias));
    let executor = Arc::new(Executor(AtomicUsize::new(0)));
    let resolver = ConfigValueResolver::with_ports(
        Arc::new(CommandConfigCache::default()),
        Arc::new(SystemConfigValueClock),
        executor.clone(),
    );
    assert_eq!(snapshot(&bare, &resolver), [("x-donor".into(), "bare".into())]);
    let observer = registry.find_exact(&"newapi".into(), &"observer".into()).unwrap().unwrap();
    assert_eq!(wire(&observer, "compactionModel"), Some(s("unit")));
    assert_eq!(snapshot(&observer, &resolver), [("x-donor".into(), "observer".into())]);
    assert_eq!(executor.0.load(Ordering::SeqCst), 0);
    // Distinct Host models may share a metadata Arc. The adapter cannot use
    // that shared pointer as an ambiguous owner when sidecars differ.
    let shared = model("newapi", "unit", HeaderSlot::Absent).spec().clone();
    let first = HostModel::new(shared.clone(), header_slot("first")).unwrap();
    let second = HostModel::new(shared, header_slot("second")).unwrap();
    let registry = StaticModelRegistry::from_inputs(
        None,
        StaticRegistryInputs { bundled_models: vec![first.clone(), second], ..Default::default() },
    )
    .unwrap();
    let all = registry.get_all().unwrap();
    assert_eq!(all.len(), 2);
    assert!(Arc::ptr_eq(&all[0], &first));
    assert!(Arc::ptr_eq(&all[0], &all[1]), "native interning picks the first row even when duplicate base rows remain");
}
