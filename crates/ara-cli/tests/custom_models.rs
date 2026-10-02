//! Grouped source-backed scenarios from fixed OMP custom-models.ts and
//! model-registry.test.ts custom merge/compat/variant suppression families.
//! Host command execution and network acceptance stay in their CLI families.

use ara_cli::{
    custom_models::*,
    model_collapse::{CollapseRuntime, VariantSpec},
    model_config_values::{
        CommandConfigCache, ConfigCommandExecutor, ConfigCommandFailure, ConfigValueContext, ConfigValueResolver,
        HeaderConfigRecord, HeaderSource, SystemConfigValueClock, resolve_config_headers,
    },
    model_identity_wire::{bundled_model_reference_index, resolve_model_reference, text},
    model_patch::{HeaderSlot, HostModel, ModelPatch},
    retry_fallback::{ConfiguredThinkingLevel, ThinkingLevel},
};
use ara_rpc::{WireString, WireValue};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
};

fn definition(value: Value, headers: HeaderSlot) -> ModelPatch {
    ModelPatch::new(VariantSpec::from_json(&value), headers).unwrap()
}

fn overlay(provider: &str, api: &str, value: Value) -> CustomModelOverlay {
    build_custom_model_overlay(
        &provider.into(),
        &"https://proxy.example.com/v1".into(),
        Some(&api.into()),
        &HeaderSlot::Absent,
        None,
        None,
        None,
        None,
        None,
        &definition(value, HeaderSlot::Absent),
    )
    .unwrap()
    .unwrap()
}

fn facts(model: &VariantSpec) -> Value {
    serde_json::from_str(&model.to_wire_json().stringify()).unwrap()
}

fn own(model: &VariantSpec, key: &str) -> bool {
    model.own_keys().iter().any(|held| held.equals_ascii(key))
}

fn config_headers(pairs: &[(&str, &str)]) -> HeaderSlot {
    HeaderSlot::Source(HeaderSource::Config(HeaderConfigRecord::from_pairs(
        pairs.iter().map(|(key, value)| ((*key).into(), (*value).into())).collect(),
    )))
}

#[test]
fn overlay_preserves_complete_fields_oauth_and_model_over_provider_precedence() {
    let provider_compat =
        VariantSpec::from_json(&json!({"supportsDeveloperRole":true,"extraBody":{"source":"provider","kept":1}}));
    let provider_remote =
        VariantSpec::from_json(&json!({"model":"provider-summary","endpoint":"provider-endpoint","enabled":true}));
    let model = definition(
        json!({"id":"complete-unknown-model","name":"Complete model","api":"openai-responses",
        "baseUrl":"https://model.example.com/v1","reasoning":false,"thinking":null,"input":["text","image"],
        "imageInputDecoder":{"kind":"marker"},"tokenizer":"cl100k_base","supportsTools":false,
        "cost":{"input":1,"output":2,"cacheRead":3,"cacheWrite":4},"contextWindow":42000,"maxTokens":8000,
        "omitMaxOutputTokens":true,"preferWebsockets":false,"contextPromotionTarget":"p/large",
        "compactionModel":"p/small","remoteCompaction":{"model":"model-summary","enabled":false},
        "premiumMultiplier":2.5,"compat":{"extraBody":{"source":"model"},"supportsDeveloperRole":false}}),
        HeaderSlot::Absent,
    );
    let built = build_custom_model_overlay(
        &"custom".into(),
        &"https://provider.example.com/v1".into(),
        Some(&"anthropic-messages".into()),
        &HeaderSlot::Absent,
        None,
        None,
        Some(&provider_compat),
        Some(&"oauth".into()),
        Some(&provider_remote),
        &model,
    )
    .unwrap()
    .unwrap();
    let value = facts(built.fields());
    assert_eq!(value["provider"], "custom");
    assert_eq!(value["api"], "openai-responses");
    assert_eq!(value["baseUrl"], "https://model.example.com/v1");
    for key in [
        "id",
        "name",
        "reasoning",
        "thinking",
        "input",
        "imageInputDecoder",
        "tokenizer",
        "supportsTools",
        "cost",
        "contextWindow",
        "maxTokens",
        "omitMaxOutputTokens",
        "preferWebsockets",
        "contextPromotionTarget",
        "compactionModel",
        "premiumMultiplier",
    ] {
        assert_eq!(value[key], facts(model.fields())[key], "overlay lost {key}");
    }
    assert_eq!(value["compat"]["extraBody"], json!({"source":"model","kept":1}));
    assert_eq!(value["compat"]["supportsDeveloperRole"], false);
    assert_eq!(
        value["remoteCompaction"],
        json!({"model":"model-summary","endpoint":"provider-endpoint","enabled":false})
    );
    assert_eq!(value["isOAuth"], true);
    assert!(!own(built.fields(), "headers") && !own(built.fields(), "apiKey"));
    assert!(matches!(built.headers(), HeaderSlot::Undefined));

    for (api, auth, expected) in [
        ("anthropic-messages", None, Some(true)),
        ("anthropic-messages", Some("apiKey"), None),
        ("anthropic-messages", Some("none"), None),
        ("openai-responses", None, None),
        ("openai-completions", Some("oauth"), Some(true)),
    ] {
        let auth = auth.map(WireString::from);
        let built = build_custom_model_overlay(
            &"custom".into(),
            &"https://proxy.example.com".into(),
            Some(&api.into()),
            &HeaderSlot::Absent,
            None,
            None,
            None,
            auth.as_ref(),
            None,
            &definition(json!({"id":"unknown"}), HeaderSlot::Absent),
        )
        .unwrap()
        .unwrap();
        assert_eq!(built.fields().get("isOAuth"), expected.map(WireValue::Bool).as_ref());
        assert!(own(built.fields(), "isOAuth"));
    }
    let missing_api = build_custom_model_overlay(
        &"custom".into(),
        &"https://proxy.example.com".into(),
        None,
        &HeaderSlot::Absent,
        None,
        None,
        None,
        None,
        None,
        &definition(json!({"id":"no-api"}), HeaderSlot::Absent),
    )
    .unwrap();
    assert!(missing_api.is_none());
}

#[test]
fn finalize_defaults_reference_policy_and_authored_undefined_null_are_distinct() {
    // Original custom gpt-5.4 replacement and standalone policy scenarios.
    for provider in ["openai", "my-proxy"] {
        let model = finalize_custom_model(
            &overlay(provider, "openai-responses", json!({"id":"gpt-5.4"})),
            CustomModelBuildOptions { use_defaults: true },
        )
        .unwrap();
        assert_eq!(facts(model.spec())["contextWindow"], 1_000_000);
        assert_eq!(facts(model.spec())["baseUrl"], "https://proxy.example.com/v1");
        let explicit = finalize_custom_model(
            &overlay(provider, "openai-responses", json!({"id":"gpt-5.4","contextWindow":256000})),
            CustomModelBuildOptions { use_defaults: true },
        )
        .unwrap();
        assert_eq!(facts(explicit.spec())["contextWindow"], 256000);
    }
    let reference = resolve_model_reference(&"gpt-5.4".into(), bundled_model_reference_index()).unwrap();
    let copied = finalize_custom_model(
        &overlay("proxy", "openai-responses", json!({"id":"gpt-5.4"})),
        CustomModelBuildOptions { use_defaults: true },
    )
    .unwrap();
    for key in ["cost", "input", "reasoning", "maxTokens", "omitMaxOutputTokens"] {
        if let Some(value) = reference.get(key) {
            assert!(copied.spec().get(key).unwrap().deep_equal(value), "reference {key}");
        }
    }
    let copilot = finalize_custom_model(
        &overlay("github-copilot", "openai-responses", json!({"id":"gpt-5.4"})),
        CustomModelBuildOptions { use_defaults: true },
    )
    .unwrap();
    assert!(copilot.spec().get("contextWindow").unwrap().deep_equal(reference.get("contextWindow").unwrap()));

    let defaults = finalize_custom_model(
        &overlay("custom", "openai-completions", json!({"id":"unrecognized-custom-zz"})),
        CustomModelBuildOptions { use_defaults: true },
    )
    .unwrap();
    let value = facts(defaults.spec());
    assert_eq!(value["name"], "unrecognized-custom-zz");
    assert_eq!(value["reasoning"], false);
    assert_eq!(value["input"], json!(["text"]));
    assert_eq!(value["cost"], json!({"input":0,"output":0,"cacheRead":0,"cacheWrite":0}));
    assert_eq!(value["contextWindow"], 128000);
    assert_eq!(value["maxTokens"], 16384);
    assert!(!own(defaults.spec(), "supportsTools"));

    let no_defaults = finalize_custom_model(
        &overlay("custom", "openai-completions", json!({"id":"unrecognized-custom-zz","name":"Minimal"})),
        CustomModelBuildOptions { use_defaults: false },
    )
    .unwrap();
    assert_eq!(no_defaults.spec().get("contextWindow"), Some(&WireValue::Null));
    assert_eq!(no_defaults.spec().get("maxTokens"), Some(&WireValue::Null));
    for key in ["cost", "input", "reasoning"] {
        assert!(own(no_defaults.spec(), key));
        assert!(no_defaults.spec().get(key).is_none(), "no defaults populated {key}");
    }
    assert!(
        finalize_custom_model(
            &overlay("custom", "openai-completions", json!({"id":"unknown-without-name"})),
            CustomModelBuildOptions { use_defaults: false }
        )
        .is_err()
    );
    let null_window = overlay("proxy", "openai-responses", json!({"id":"gpt-5.4","contextWindow":null}));
    let null_window = finalize_custom_model(&null_window, CustomModelBuildOptions { use_defaults: true }).unwrap();
    assert!(null_window.spec().get("contextWindow").unwrap().deep_equal(reference.get("contextWindow").unwrap()));
}

#[test]
fn reference_thinking_is_only_inherited_within_its_provider_and_explicit_values_win() {
    let index = bundled_model_reference_index();
    let reference = index
        .exact
        .values()
        .find(|model| {
            model.get_path(&["thinking".into(), "effortRouting".into()]).is_some()
                && text(model, "id").is_some_and(|id| !id.units().contains(&47))
        })
        .expect("fixed bundled catalog contains a provider-local thinking route");
    let id = text(reference, "id").unwrap();
    let provider = text(reference, "provider").unwrap();
    let api = text(reference, "api").unwrap();
    let make = |provider: &WireString, definition: Value| {
        let model = build_custom_model_overlay(
            provider,
            &"https://proxy.example.com/v1".into(),
            Some(&api),
            &HeaderSlot::Absent,
            None,
            None,
            None,
            None,
            None,
            &definition_fn(definition),
        )
        .unwrap()
        .unwrap();
        finalize_custom_model(&model, CustomModelBuildOptions { use_defaults: true }).unwrap()
    };
    let model_definition = json!({"id":id.to_utf8().unwrap()});
    let same = make(&provider, model_definition.clone());
    let foreign = make(&"different-custom-provider".into(), model_definition.clone());
    let route = ["thinking".into(), "effortRouting".into()];
    assert!(same.spec().get_path(&route).unwrap().deep_equal(reference.get_path(&route).unwrap()));
    assert!(foreign.spec().get_path(&route).is_none(), "reference transport routing escaped its provider");
    let authored = make(
        &provider,
        json!({"id":id.to_utf8().unwrap(),"thinking":{"mode":"budget","efforts":["high"],
        "effortRouting":{"high":"authored-high-wire"}}}),
    );
    assert_eq!(
        authored.spec().get_path(&["thinking".into(), "effortRouting".into(), "high".into()]),
        Some(&WireValue::String("authored-high-wire".into()))
    );
    let disabled = make(&provider, json!({"id":id.to_utf8().unwrap(),"thinking":null}));
    assert!(disabled.spec().get_path(&route).is_none());
}

fn definition_fn(value: Value) -> ModelPatch {
    definition(value, HeaderSlot::Absent)
}

struct Tokens {
    calls: Mutex<Vec<String>>,
    outputs: Mutex<HashMap<String, String>>,
}
impl ConfigCommandExecutor for Tokens {
    fn directory_is_enterable(&self, _: &Path) -> bool {
        true
    }
    fn execute(&self, command: &str, _: &Path) -> Result<String, ConfigCommandFailure> {
        self.calls.lock().unwrap().push(command.into());
        self.outputs.lock().unwrap().get(command).cloned().ok_or(ConfigCommandFailure::NonZeroExit)
    }
}

#[test]
fn live_headers_keep_raw_sources_refresh_and_generated_auth_without_public_metadata() {
    let tokens = Arc::new(Tokens {
        calls: Mutex::new(Vec::new()),
        outputs: Mutex::new(HashMap::from([
            ("provider-header".into(), "provider-stale".into()),
            ("model-header".into(), "model-stale".into()),
            ("bearer".into(), "synthetic-stale-secret".into()),
        ])),
    });
    let resolver = ConfigValueResolver::with_ports(
        Arc::new(CommandConfigCache::default()),
        Arc::new(SystemConfigValueClock),
        tokens.clone(),
    );
    let environment = |_: &str| None;
    let context = ConfigValueContext { project_dir: Path::new("."), environment: &environment };
    let provider = config_headers(&[("X-Route", "!provider-header"), ("X-Provider", "!provider-header")]);
    let model = definition(json!({"id":"header-test"}), config_headers(&[("X-Route", "!model-header")]));
    let overlay = build_custom_model_overlay(
        &"custom".into(),
        &"https://proxy.example.com".into(),
        Some(&"openai-completions".into()),
        &provider,
        Some("!bearer"),
        Some(true),
        None,
        None,
        None,
        &model,
    )
    .unwrap()
    .unwrap();
    assert!(tokens.calls.lock().unwrap().is_empty(), "composing live headers executed commands");
    let finalized = finalize_custom_model(&overlay, CustomModelBuildOptions { use_defaults: true }).unwrap();
    let source = finalized.headers().as_source().unwrap();
    let first = resolve_config_headers(Some(source), &resolver, &context).unwrap();
    assert_eq!(first.get("X-Route"), Some("model-stale"));
    assert_eq!(first.get("X-Provider"), Some("provider-stale"));
    assert_eq!(first.get("Authorization"), Some("Bearer synthetic-stale-secret"));
    assert_eq!(tokens.calls.lock().unwrap().as_slice(), ["provider-header", "model-header", "bearer"]);
    for (command, fresh) in
        [("provider-header", "provider-fresh"), ("model-header", "model-fresh"), ("bearer", "synthetic-fresh-secret")]
    {
        tokens.outputs.lock().unwrap().insert(command.into(), fresh.into());
        resolver.invalidate_command_config(Some(&format!("!{command}")));
    }
    let wrapped = merge_auth_header_sources(&[finalized.headers().clone()], None, None);
    let fresh = resolve_config_headers(wrapped.as_source(), &resolver, &context).unwrap();
    assert_eq!(fresh.get("X-Route"), Some("model-fresh"));
    assert_eq!(fresh.get("X-Provider"), Some("provider-fresh"));
    assert_eq!(fresh.get("Authorization"), Some("Bearer synthetic-fresh-secret"));
    let serialized = finalized.spec().to_wire_json().stringify();
    for private in [
        "!provider-header",
        "!model-header",
        "!bearer",
        "synthetic-stale-secret",
        "synthetic-fresh-secret",
        "headers",
        "apiKey",
    ] {
        assert!(!serialized.contains(private));
    }
    let auth_only = merge_auth_header_sources(&[], Some(true), Some("!bearer"));
    assert_eq!(
        resolve_config_headers(auth_only.as_source(), &resolver, &context).unwrap().get("Authorization"),
        Some("Bearer synthetic-fresh-secret")
    );
    assert!(matches!(merge_auth_header_sources(&[], Some(false), Some("!bearer")), HeaderSlot::Undefined));
}

#[test]
fn suppression_grammar_and_retired_overrides_preserve_literal_and_live_id_guards() {
    let live = |_: &WireString, id: &WireString| id.equals_ascii("glm-4.7:max") || id.equals_ascii("m:auto");
    let mut runtime = CollapseRuntime::new().unwrap();
    for (input, expected) in [
        ("  google-antigravity/gemini-3-pro-high:high  ", "google-antigravity/gemini-3-pro"),
        ("google-antigravity/gemini-3-pro-low:auto", "google-antigravity/gemini-3-pro"),
        ("proxy/glm-4.7:max", "proxy/glm-4.7:max"),
        ("proxy/m:auto", "proxy/m:auto"),
        ("proxy/m:high", "proxy/m"),
        ("provider/path/model:xhi", "provider/path/model"),
        (" invalid-bare-id ", "invalid-bare-id"),
        ("/bad", "/bad"),
        ("\u{FEFF}\u{3000}", ""),
        ("provider/", "provider/"),
    ] {
        assert_eq!(normalize_suppressed_selector(&input.into(), &mut runtime, Some(&live)).unwrap(), expected.into());
    }
    let strict_literal = |_: &WireString, _: &WireString| true;
    let strict = parse_model_string(
        &"p/m:high".into(),
        ModelStringParseOptions {
            allow_max_suffix: true,
            allow_auto_alias: true,
            is_literal_model_id: Some(&strict_literal),
        },
    )
    .unwrap();
    assert_eq!(strict.id, "m".into());
    assert_eq!(strict.thinking_level, Some(ConfiguredThinkingLevel::Concrete(ThinkingLevel::High)));
    let max = parse_model_string(&"p/m:max".into(), ModelStringParseOptions::default()).unwrap();
    assert_eq!(max.id, "m:max".into());
    let auto = parse_model_string(
        &"p/m:auto".into(),
        ModelStringParseOptions { allow_auto_alias: true, ..Default::default() },
    )
    .unwrap();
    assert_eq!(auto.id, "m".into());
    assert_eq!(auto.thinking_level, Some(ConfiguredThinkingLevel::Auto));
    let lone = WireString::from_units(vec![0xd800, 47, 0xdc00, 58, 104, 105, 103, 104]);
    let parsed = parse_model_string(&lone, ModelStringParseOptions::default()).unwrap();
    assert_eq!(parsed.provider.units(), [0xd800]);
    assert_eq!(parsed.id.units(), [0xdc00]);

    let model = HostModel::new(
        Arc::new(VariantSpec::from_json(&json!({"provider":"google-antigravity","id":"gemini-3-pro"}))),
        HeaderSlot::Absent,
    )
    .unwrap();
    let alias_patch = definition_fn(json!({"contextWindow":222222}));
    let mut overrides = HashMap::from([("gemini-3-pro-high".into(), alias_patch)]);
    let selected =
        resolve_model_override_with_aliases(&overrides, &model, &mut runtime, &|_, _| false).unwrap().unwrap();
    assert_eq!(selected.fields().get("contextWindow"), Some(&WireValue::Number(222222.0)));
    assert!(
        resolve_model_override_with_aliases(&overrides, &model, &mut runtime, &|_, id| id
            .equals_ascii("gemini-3-pro-high"))
        .unwrap()
        .is_none()
    );
    overrides.insert("gemini-3-pro".into(), definition_fn(json!({"contextWindow":333333})));
    let direct = resolve_model_override_with_aliases(&overrides, &model, &mut runtime, &|_, _| true).unwrap().unwrap();
    assert_eq!(direct.fields().get("contextWindow"), Some(&WireValue::Number(333333.0)));

    // Existing collapse registration supplies dynamic aliases; do not create a
    // parallel alias table just for custom models.
    let specs = [
        Arc::new(VariantSpec::from_json(&json!({"provider":"custom-pair","id":"pair-model",
        "api":"openai-completions","name":"Pair","reasoning":false}))),
        Arc::new(VariantSpec::from_json(&json!({"provider":"custom-pair","id":"pair-model-thinking",
        "api":"openai-completions","name":"Pair thinking","reasoning":true}))),
    ];
    let collapsed = runtime.collapse_variants(&specs, None).unwrap();
    assert_eq!(collapsed.len(), 1);
    assert_eq!(
        normalize_suppressed_selector(&"custom-pair/pair-model-thinking".into(), &mut runtime, None).unwrap(),
        "custom-pair/pair-model".into()
    );
}
