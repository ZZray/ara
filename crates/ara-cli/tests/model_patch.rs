//! Native registry scenario families for fixed OMP model-patch.ts. These
//! deterministic tests do not establish real provider/process acceptance.

use ara_cli::{
    model_collapse::{CollapseError, VariantSpec},
    model_config_values::{
        CommandConfigCache, ConfigCommandExecutor, ConfigCommandFailure, ConfigValueClock, ConfigValueContext,
        ConfigValueResolver, HeaderConfigRecord, HeaderSource,
    },
    model_patch::{
        HeaderSlot, HostModel, HostModelRef, ModelPatch, ModelTransportPolicy, OrderedProviderSet, ProviderOverride,
        apply_model_override, apply_model_patch, authoritative_runtime_catalog_providers, build_host_model,
        drop_provider_models, merge_by_model_key, merge_compat, merge_compat_refs, merge_discovered_model,
        merge_provider_remote_compaction_config, merge_remote_compaction_config, merge_remote_compaction_config_refs,
        providers_with_authoritative_project_catalog,
    },
};
use ara_rpc::{WireString, WireValue};
use serde_json::json;
use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

fn spec(value: serde_json::Value) -> VariantSpec {
    VariantSpec::from_json(&value)
}

fn model(provider: &str, id: &str, headers: HeaderSlot) -> HostModelRef {
    build_host_model(
        &spec(json!({
            "provider": provider, "id": id, "name": id, "api": "openai-completions",
            "baseUrl": "https://example.test/v1", "reasoning": false, "input": ["text"],
            "cost": {"input":11,"output":22,"cacheRead":33,"cacheWrite":44,"longContext":{"input":55}},
            "contextWindow":128000, "maxTokens":8192,
        })),
        headers,
    )
    .unwrap()
}

fn changed(model: &HostModelRef, edit: impl FnOnce(&mut VariantSpec)) -> HostModelRef {
    let mut fields = model.spec().as_ref().clone();
    edit(&mut fields);
    model.with_spec(Arc::new(fields)).unwrap()
}

fn record(pairs: &[(&str, &str)]) -> HeaderConfigRecord {
    HeaderConfigRecord::from_pairs(pairs.iter().map(|(key, value)| ((*key).into(), (*value).into())).collect())
}

fn header_slot(record: &HeaderConfigRecord) -> HeaderSlot {
    HeaderSlot::Source(HeaderSource::Config(record.clone()))
}

fn wire(model: &HostModelRef, path: &[&str]) -> Option<WireValue> {
    model.spec().get_path(&path.iter().map(|part| WireString::from(*part)).collect::<Vec<_>>()).cloned()
}

fn s(value: &str) -> WireValue {
    WireValue::String(value.into())
}

struct Clock;
impl ConfigValueClock for Clock {
    fn now_millis(&self) -> u64 {
        0
    }
}
struct Executor {
    output: Mutex<String>,
    calls: AtomicUsize,
}
impl ConfigCommandExecutor for Executor {
    fn directory_is_enterable(&self, _: &Path) -> bool {
        true
    }
    fn execute(&self, command: &str, _: &Path) -> Result<String, ConfigCommandFailure> {
        assert_eq!(command, "synthetic-credential");
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.output.lock().unwrap().clone())
    }
}

fn snapshot(
    model: &HostModelRef,
    resolver: &ConfigValueResolver,
    context: &ConfigValueContext<'_>,
) -> Vec<(String, String)> {
    match model.headers() {
        HeaderSlot::Source(HeaderSource::Live(headers)) => headers.snapshot(resolver, context).unwrap().into_pairs(),
        _ => panic!("expected an opaque live source"),
    }
}

#[test]
fn discovery_refresh_preserves_endpoint_transport_identity_and_live_credential_ownership() {
    let existing_headers = record(&[("x-bundled", "1"), ("x-shared", "existing")]);
    let discovered_headers = record(&[("x-tp", "1"), ("x-shared", "discovered"), ("Authorization", "Bearer stale")]);
    let provider_headers = record(&[("x-shared", "HEADER_ENV"), ("x-key", "!synthetic-credential")]);
    let existing = changed(&model("xiaomi", "mimo-v2.5", header_slot(&existing_headers)), |fields| {
        fields.set("baseUrl", s("https://api.xiaomimimo.com/v1"));
        fields.set("transport", s("pi-native"));
        fields.set("supportsTools", WireValue::Bool(true));
        fields.set("remoteCompaction", spec(json!({"existing":1,"shared":"existing"})).value);
    });
    let discovered = changed(&model("xiaomi", "mimo-v2.5", header_slot(&discovered_headers)), |fields| {
        fields.set("baseUrl", s("https://token-plan-sgp.xiaomimimo.com/v1"));
        fields.set("transport", s("discovery-transport"));
        fields.set_undefined("supportsTools");
        fields.set("remoteCompaction", spec(json!({"discovered":2,"shared":"discovered"})).value);
        fields.set("compatConfig", spec(json!({"extraBody":{"discovered":true}})).value);
    });
    let untouched = merge_discovered_model(&discovered, None, None).unwrap();
    assert!(Arc::ptr_eq(&untouched, &discovered));
    let merged = merge_discovered_model(&discovered, Some(&existing), None).unwrap();
    assert_eq!(wire(&merged, &["baseUrl"]), Some(s("https://token-plan-sgp.xiaomimimo.com/v1")));
    assert_eq!(wire(&merged, &["transport"]), Some(s("pi-native")));
    assert_eq!(wire(&merged, &["supportsTools"]), Some(WireValue::Bool(true)));
    let missing = changed(&discovered, |fields| fields.set_undefined("baseUrl"));
    let fallback = merge_discovered_model(&missing, Some(&existing), None).unwrap();
    assert_eq!(wire(&fallback, &["baseUrl"]), Some(s("https://api.xiaomimimo.com/v1")));
    let explicit = ProviderOverride::new(
        spec(json!({"baseUrl":"https://custom.test/v1", "transport":"user-transport", "authHeader":true,
            "compat":{"extraBody":{"provider":true}}, "remoteCompaction":{"provider":3,"shared":"provider"},
            "guardrailIdentifier":"guard", "guardrailVersion":"1", "guardrailTrace":"enabled",
            "requestMetadata":{"source":"synthetic"}})),
        header_slot(&provider_headers),
        Some("!synthetic-credential".into()),
    )
    .unwrap();
    assert!(explicit.fields().get("guardrailIdentifier").is_some());
    assert!(explicit.fields().get("headers").is_none());
    assert!(explicit.fields().get("apiKey").is_none());
    let merged = merge_discovered_model(&discovered, Some(&existing), Some(&explicit)).unwrap();
    assert_eq!(wire(&merged, &["baseUrl"]), Some(s("https://custom.test/v1")));
    assert_eq!(wire(&merged, &["transport"]), Some(s("user-transport")));
    assert_eq!(wire(&merged, &["remoteCompaction", "shared"]), Some(s("discovered")));
    for key in ["existing", "discovered", "provider"] {
        assert!(wire(&merged, &["remoteCompaction", key]).is_some());
    }
    for key in ["discovered", "provider"] {
        assert_eq!(wire(&merged, &["compatConfig", "extraBody", key]), Some(WireValue::Bool(true)));
    }
    let executor = Arc::new(Executor { output: Mutex::new("first-token".into()), calls: AtomicUsize::new(0) });
    let resolver =
        ConfigValueResolver::with_ports(Arc::new(CommandConfigCache::default()), Arc::new(Clock), executor.clone());
    let environment = |name: &str| (name == "HEADER_ENV").then(|| "environment-header".into());
    let context = ConfigValueContext { project_dir: Path::new("."), environment: &environment };
    assert_eq!(
        snapshot(&merged, &resolver, &context),
        [
            ("x-bundled".into(), "1".into()),
            ("x-shared".into(), "environment-header".into()),
            ("x-tp".into(), "1".into()),
            ("Authorization".into(), "Bearer first-token".into()),
            ("x-key".into(), "first-token".into()),
        ]
    );
    assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
    provider_headers.set("x-shared", "mutated-raw");
    *executor.output.lock().unwrap() = "rotated-token".into();
    resolver.invalidate_command_config(Some("!synthetic-credential"));
    let refreshed = snapshot(&merged, &resolver, &context);
    assert!(refreshed.contains(&("x-shared".into(), "mutated-raw".into())));
    assert!(refreshed.contains(&("Authorization".into(), "Bearer rotated-token".into())));
    assert_eq!(executor.calls.load(Ordering::SeqCst), 2);
    let auth_only = ProviderOverride::new(
        spec(json!({"authHeader":true})),
        HeaderSlot::Absent,
        Some("!synthetic-credential".into()),
    )
    .unwrap();
    let without_existing = merge_discovered_model(&discovered, None, Some(&auth_only)).unwrap();
    assert_eq!(wire(&without_existing, &["transport"]), Some(s("discovery-transport")));
    assert!(
        snapshot(&without_existing, &resolver, &context)
            .contains(&("Authorization".into(), "Bearer rotated-token".into()))
    );
    let last_null = changed(&discovered, |fields| fields.set("transport", WireValue::Null));
    let old_no_transport = changed(&existing, |fields| fields.remove("transport"));
    assert_eq!(
        wire(&merge_discovered_model(&last_null, Some(&old_no_transport), None).unwrap(), &["transport"]),
        Some(WireValue::Null)
    );
}

#[test]
fn registry_patch_covers_authored_fields_cost_corrections_thinking_and_transport_policies() {
    let base_headers = record(&[("x-bundled", "base")]);
    let extra_headers = record(&[("x-user", "user")]);
    let base = changed(&model("openai", "gpt-5.4", header_slot(&base_headers)), |fields| {
        fields.set("api", s("openai-responses"));
        fields.set("compatConfig", spec(json!({"extraBody":{"base":true}})).value);
        fields.set("remoteCompaction", spec(json!({"base":1,"shared":"base"})).value);
        fields.set(
            "cost",
            spec(json!({"input":11,"output":22,"cacheRead":33,"cacheWrite":44,
            "longContext":{"input":55},"discardedCostField":99}))
            .value,
        );
    });
    let mut fields = spec(json!({
        "name":"Configured GPT", "reasoning":true,
        "thinking":{"mode":"effort","efforts":["high","low"]}, "input":[],
        "imageInputDecoder":"custom-decoder", "tokenizer":"custom-tokenizer", "supportsTools":false,
        "cost":{"input":0,"output":null,"cacheRead":0,"cacheWrite":0}, "contextWindow":256000,"maxTokens":0,
        "omitMaxOutputTokens":false,"preferWebsockets":false,"compat":{"extraBody":{"patch":true}},
        "contextPromotionTarget":"next","compactionModel":"small","remoteCompaction":{"shared":"patch"},
        "premiumMultiplier":0,"ignoredExtension":"not-a-model-field",
    }));
    let malformed = WireString::from_units(vec![0xD800, 0x0061]);
    fields.set("contextPromotionTarget", WireValue::String(malformed.clone()));
    let patch = ModelPatch::new(fields, header_slot(&extra_headers)).unwrap();
    let patched = apply_model_override(&base, &patch).unwrap();
    for key in [
        "name",
        "reasoning",
        "thinking",
        "input",
        "imageInputDecoder",
        "tokenizer",
        "supportsTools",
        "contextWindow",
        "maxTokens",
        "omitMaxOutputTokens",
        "preferWebsockets",
        "contextPromotionTarget",
        "compactionModel",
        "premiumMultiplier",
    ] {
        assert_eq!(wire(&patched, &[key]), patch.fields().get(key).cloned(), "authored {key}");
    }
    assert!(wire(&patched, &["ignoredExtension"]).is_none());
    assert_eq!(
        wire(&patched, &["cost"]),
        Some(
            spec(json!({
                "input":0,"output":22,"cacheRead":0,"cacheWrite":0,"longContext":{"input":55}
            }))
            .value
        )
    );
    assert_eq!(wire(&patched, &["contextPromotionTarget"]), Some(WireValue::String(malformed)));
    assert_eq!(wire(&patched, &["remoteCompaction", "base"]), Some(WireValue::Number(1.0)));
    assert_eq!(wire(&patched, &["remoteCompaction", "shared"]), Some(s("patch")));
    assert_eq!(wire(&patched, &["compatConfig", "extraBody", "base"]), Some(WireValue::Bool(true)));
    assert_eq!(wire(&patched, &["compatConfig", "extraBody", "patch"]), Some(WireValue::Bool(true)));
    let resolver = ConfigValueResolver::isolated();
    let no_env = |_: &str| None;
    let context = ConfigValueContext { project_dir: Path::new("."), environment: &no_env };
    assert_eq!(
        snapshot(&patched, &resolver, &context),
        [("x-bundled".into(), "base".into()), ("x-user".into(), "user".into())]
    );
    extra_headers.set("x-user", "mutated");
    assert!(snapshot(&patched, &resolver, &context).contains(&("x-user".into(), "mutated".into())));
    let mut undefined = spec(json!({}));
    undefined.set_undefined("name");
    undefined.set_undefined("input");
    undefined.set_undefined("contextWindow");
    let undefined = ModelPatch::new(undefined, HeaderSlot::Undefined).unwrap();
    let unchanged = apply_model_override(&patched, &undefined).unwrap();
    assert_eq!(wire(&unchanged, &["name"]), wire(&patched, &["name"]));
    assert!(matches!(unchanged.headers(), HeaderSlot::Source(HeaderSource::Live(_))));
    let plain = model("custom", "opaque-model", HeaderSlot::Absent);
    let undefined_plain = apply_model_override(&plain, &undefined).unwrap();
    for key in ["name", "input", "contextWindow"] {
        assert_eq!(wire(&undefined_plain, &[key]), wire(&plain, &[key]));
    }
    let replaced = apply_model_patch(&base, &undefined, ModelTransportPolicy::Replace).unwrap();
    assert!(matches!(replaced.headers(), HeaderSlot::Undefined));
    assert!(wire(&replaced, &["compatConfig"]).is_none());
    let absent = ModelPatch::new(spec(json!({})), HeaderSlot::Absent).unwrap();
    assert!(matches!(
        apply_model_patch(&base, &absent, ModelTransportPolicy::Replace).unwrap().headers(),
        HeaderSlot::Undefined
    ));
    let null = ModelPatch::new(spec(json!({})), HeaderSlot::Null).unwrap();
    assert!(matches!(
        apply_model_patch(&base, &null, ModelTransportPolicy::Replace).unwrap().headers(),
        HeaderSlot::Null
    ));
    let raw_patch = ModelPatch::new(spec(json!({})), header_slot(&extra_headers)).unwrap();
    assert!(matches!(
        apply_model_patch(&base, &raw_patch, ModelTransportPolicy::Replace).unwrap().headers(),
        HeaderSlot::Source(HeaderSource::Config(_))
    ));
    for fields in [
        json!({"reasoning":false,"thinking":{"mode":"effort","efforts":["high"]}}),
        json!({"reasoning":true,"thinking":{"mode":"effort","efforts":["high"]},"compat":{"supportsReasoningEffort":false}}),
    ] {
        let suppress = ModelPatch::new(spec(fields), HeaderSlot::Absent).unwrap();
        assert!(wire(&apply_model_override(&base, &suppress).unwrap(), &["thinking"]).is_none());
    }
    let no_long_context = ModelPatch::new(spec(json!({"cost":{"longContext":false}})), HeaderSlot::Absent).unwrap();
    let no_long_context = apply_model_override(&base, &no_long_context).unwrap();
    assert!(wire(&no_long_context, &["cost", "longContext"]).is_none());
    assert!(!HeaderSlot::Absent.is_own());
    assert!(HeaderSlot::Undefined.is_own());
    assert!(!HeaderSlot::Undefined.is_defined());
    assert!(HeaderSlot::Null.is_defined());
    let mut forbidden = spec(json!({}));
    forbidden.set_undefined("headers");
    assert!(HostModel::new(Arc::new(forbidden.clone()), HeaderSlot::Absent).is_err());
    assert!(ModelPatch::new(forbidden, HeaderSlot::Absent).is_err());
    assert!(ProviderOverride::new(spec(json!({"apiKey":"synthetic-only"})), HeaderSlot::Absent, None).is_err());
}

#[test]
fn compat_and_remote_compaction_keep_lossless_presence_order_and_distinct_merge_depth() {
    let base = VariantSpec::from_wire(
        WireValue::parse(r#"{"nested":{"keep":1,"replace":2},"array":[1,2],"scalar":3,"tail":4}"#).unwrap(),
    );
    let mut override_ = VariantSpec::from_wire(WireValue::parse(r#"{"nested":{"replace":null,"undefined":null,"utf16":"\ud800"},"array":[9],"scalar":{"new":true},"added":5}"#).unwrap());
    override_.undefined_paths.push(vec!["nested".into(), "undefined".into()]);
    let merged = merge_compat(Some(&base), Some(&override_)).unwrap();
    assert_eq!(merged.own_keys(), ["nested", "array", "scalar", "tail", "added"].map(WireString::from));
    assert_eq!(merged.get_path(&["nested".into(), "keep".into()]), Some(&WireValue::Number(1.0)));
    assert_eq!(merged.get_path(&["nested".into(), "replace".into()]), Some(&WireValue::Null));
    assert!(merged.get_path(&["nested".into(), "undefined".into()]).is_none());
    assert!(merged.undefined_paths.contains(&vec!["nested".into(), "undefined".into()]));
    assert_eq!(
        merged.get_path(&["nested".into(), "utf16".into()]),
        Some(&WireValue::String(WireString::from_units(vec![0xD800])))
    );
    assert_eq!(merged.get("array"), Some(&WireValue::Array(vec![WireValue::Number(9.0)])));
    assert_eq!(merged.get_path(&["scalar".into(), "new".into()]), Some(&WireValue::Bool(true)));
    let null = VariantSpec::from_wire(WireValue::Null);
    assert!(merge_compat(None, Some(&null)).is_none());
    assert_eq!(merge_compat(Some(&base), Some(&null)).unwrap().to_wire_json(), base.to_wire_json());
    let base_ref = Arc::new(base.clone());
    assert!(Arc::ptr_eq(&merge_compat_refs(Some(&base_ref), None).unwrap(), &base_ref));
    assert!(Arc::ptr_eq(&merge_compat_refs(None, Some(&base_ref)).unwrap(), &base_ref));
    assert!(Arc::ptr_eq(&merge_remote_compaction_config_refs(None, Some(&base_ref)).unwrap(), &base_ref));
    let remote = merge_remote_compaction_config(Some(&base), Some(&override_)).unwrap();
    assert!(remote.get_path(&["nested".into(), "keep".into()]).is_none());
    assert!(remote.undefined_paths.contains(&vec!["nested".into(), "undefined".into()]));
    let model = spec(json!({"shared":"model","model":true}));
    let provider = spec(json!({"shared":"provider","provider":true}));
    let remote = merge_provider_remote_compaction_config(Some(&model), Some(&provider)).unwrap();
    assert_eq!(remote.get("shared"), Some(&s("model")));
    assert_eq!(remote.own_keys(), ["shared", "provider", "model"].map(WireString::from));
    assert_eq!(merge_remote_compaction_config(None, Some(&null)).unwrap().value, WireValue::Null);
}

#[test]
fn provider_catalog_sets_and_ordered_key_merge_preserve_duplicates_callbacks_and_refs() {
    assert_eq!(
        authoritative_runtime_catalog_providers().iter().cloned().collect::<Vec<_>>(),
        [
            "abliteration",
            "aiand",
            "aimlapi",
            "alibaba-token-plan",
            "baseten",
            "bedrock-mantle",
            "deepinfra",
            "devin",
            "cline-pass",
            "gitlab-duo-agent",
            "gmi-cloud",
            "novita",
            "opencode-go",
            "opencode-zen",
            "sakana",
            "siliconflow",
            "siliconflow-cn",
            "synthetic",
            "umans",
            "coreweave",
            "yolo-auto",
            "zhipu-coding-plan",
        ]
        .map(WireString::from)
    );
    let vertex = model("google-vertex", "model", HeaderSlot::Absent);
    let express = changed(&vertex, |fields| fields.set("baseUrl", s("https://vertex.test/endpoints/openapi/model")));
    let wrong_api = changed(&express, |fields| fields.set("api", s("google-vertex")));
    let wrong_provider = changed(&express, |fields| fields.set("provider", s("openai")));
    let catalog = providers_with_authoritative_project_catalog(&[
        vertex.clone(),
        wrong_api.clone(),
        wrong_provider.clone(),
        express.clone(),
        express.clone(),
    ]);
    assert_eq!(catalog.iter().cloned().collect::<Vec<_>>(), [WireString::from("google-vertex")]);
    let filtered = drop_provider_models(&[vertex, wrong_api, wrong_provider.clone(), express], &catalog);
    assert_eq!(filtered.len(), 1);
    assert!(Arc::ptr_eq(&filtered[0], &wrong_provider));
    let mut providers = OrderedProviderSet::default();
    assert!(providers.insert("b".into()));
    assert!(providers.insert("a".into()));
    assert!(!providers.insert("b".into()));
    assert_eq!(providers.iter().cloned().collect::<Vec<_>>(), [WireString::from("b"), WireString::from("a")]);
    let entry = |provider: &str, id: &str| spec(json!({"provider":provider,"id":id}));
    let unbuilt = |provider: &str, id: &str| HostModel::new(Arc::new(entry(provider, id)), HeaderSlot::Absent).unwrap();
    let first = unbuilt("p", "a");
    let last = unbuilt("p", "a");
    let other = unbuilt("q", "a");
    let collision = unbuilt("p\0q", "r");
    let key_of = |entry: &VariantSpec| {
        (
            entry.get("provider").unwrap().as_string().unwrap().clone(),
            entry.get("id").unwrap().as_string().unwrap().clone(),
        )
    };
    let mut seen = Vec::new();
    let merged = merge_by_model_key(
        &[first.clone(), last, other.clone(), collision],
        &[entry("p", "a"), entry("new", "n"), entry("p", "a"), entry("p", "q\0r")],
        key_of,
        |existing, entry| {
            seen.push(existing.is_some());
            let mut out = entry.clone();
            let count =
                existing.and_then(|model| model.spec().get("count")).and_then(WireValue::as_number).unwrap_or(0.0)
                    + 1.0;
            out.set("count", WireValue::Number(count));
            HostModel::new(Arc::new(out), HeaderSlot::Absent)
        },
    )
    .unwrap();
    assert_eq!(seen, [true, false, true, true]);
    assert_eq!(merged.len(), 5);
    assert!(Arc::ptr_eq(&merged[0], &first));
    assert!(Arc::ptr_eq(&merged[2], &other));
    assert_eq!(wire(&merged[1], &["count"]), Some(WireValue::Number(2.0)));
    assert_eq!(wire(&merged[3], &["provider"]), Some(s("p")));
    assert_eq!(wire(&merged[4], &["provider"]), Some(s("new")));
    let error =
        merge_by_model_key(&[], &[entry("x", "y")], key_of, |_, _| Err(CollapseError::new("callback failure".into())));
    assert_eq!(error.err().unwrap().to_string(), "callback failure");
    let owned = model("xiaomi", "mimo-v2.5", header_slot(&record(&[("x-owner", "donor")])));
    let rebuilt = owned.with_spec(Arc::new(owned.spec().as_ref().clone())).unwrap();
    assert!(!Arc::ptr_eq(&owned, &rebuilt));
    assert!(matches!(rebuilt.headers(), HeaderSlot::Source(HeaderSource::Config(_))));
    assert!(Arc::ptr_eq(owned.spec(), &owned.spec().clone()));
}
