//! Module scenarios for explicit daily CLI configuration. Socket/login/session
//! scenarios belong to the CLI/auth modules; these use the real file loader and
//! the actual native request encoders. Process policy mutation is isolated in
//! a Windows child process, never the parallel parent test process.

use ara_ai::providers::{openai_completions as chat, openai_responses as responses};
use ara_ai::{Context, DeveloperMessage, Message, UserContent, UserMessage};
use ara_cli::daily_model_config::{
    DailyApi, DailyAuthSource, DailyOverrides, load_daily_config, resolve_daily_fallback_selection,
    resolve_daily_promotion_selection, resolve_daily_selection,
};
use ara_cli::model_route::{CredentialIdentity, FixedRequestAuth, PreparedRoute, ProtocolOptions};
use ara_cli::models_config::ModelsConfig;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

fn no_env(_: &str) -> Option<String> {
    None
}

fn config(provider: Value) -> ModelsConfig {
    ModelsConfig::validate(json!({"providers":{"custom":provider}})).unwrap()
}

fn custom(api: &str) -> Value {
    json!({
        "api":api,"baseUrl":"https://config.example/v1","apiKey":"FIXTURE_KEY",
        "models":[{"id":"group/exact-model","maxTokens":512,"input":["text"]}]
    })
}

fn selected() -> DailyOverrides {
    DailyOverrides { provider: Some("custom".into()), model: Some("group/exact-model".into()), ..Default::default() }
}

fn context() -> Context {
    Context {
        system_prompt: vec!["Be concise".into()],
        messages: vec![Message::User(UserMessage::text("hello"))],
        tools: None,
    }
}

#[test]
fn selected_sparse_context_metadata_follows_capability_donor_not_resolved_defaults_group() {
    use ara_cli::append_only_context_mode::{
        AppendOnlyContextMode as Mode, AppendOnlyContextModel, should_enable_append_only_context,
    };
    use ara_cli::daily_model_config::resolve_daily_selection_with_reference;
    use ara_cli::model_collapse::VariantSpec;
    use ara_cli::model_patch::{HeaderSlot, HostModel};
    for authored in [None, Some(false), Some(true)] {
        let mut donor = json!({"id":"context-donor", "requestModelId":"context-wire-id",
            "provider":"custom", "api":"openai-completions", "baseUrl":"https://remote.example/v1",
            "reasoning":false, "supportsTools":true, "compat":{"supportsStore":true}});
        if let Some(authored) = authored {
            donor["compatConfig"] = json!({"supportsStore":authored});
        }
        let reference = HostModel::new(Arc::new(VariantSpec::from_json(&donor)), HeaderSlot::Absent).unwrap();
        let cfg = config(json!({"api":"openai-completions", "baseUrl":"https://remote.example/v1",
            "apiKey":"FIXTURE_KEY", "models":[{"id":"context-donor", "reasoning":false}]}));
        let cli = DailyOverrides {
            provider: Some("custom".into()),
            model: Some("context-donor".into()),
            ..Default::default()
        };
        let selection = resolve_daily_selection_with_reference(Some(&cfg), &cli, &no_env, &reference).unwrap();
        assert_eq!(selection.model.id, "context-wire-id");
        assert_eq!(selection.metadata["catalogModelId"], "context-donor");
        assert_eq!(selection.metadata["supportsTools"], true);
        assert_eq!(
            selection
                .metadata
                .get("compatConfig")
                .and_then(|compat| compat.get("supportsStore"))
                .and_then(Value::as_bool),
            authored
        );
        let selected = AppendOnlyContextModel::from_selected(&selection.model, Some(&selection.metadata));
        assert_eq!(should_enable_append_only_context(Mode::Auto, Some(&selected)), authored == Some(true));
    }
}

#[test]
fn selected_catalog_freeform_capability_reaches_protocol_and_metadata_group() {
    if observe_process_schema_policy_child() {
        return;
    }
    observe_no_strict_daily_policy();
    use ara_ai::providers::openai_codex_responses as codex;
    use ara_ai::{CustomToolFormat, CustomToolSyntax, Tool};
    use ara_cli::daily_model_config::resolve_daily_selection_with_reference;
    use ara_cli::model_collapse::VariantSpec;
    use ara_cli::model_patch::{HeaderSlot, HostModel};
    let edit = Tool {
        name: "edit".into(),
        description: "edit".into(),
        parameters: json!({"type":"object","properties":{"input":{"type":"string"}}}),
        custom_format: Some(CustomToolFormat { syntax: CustomToolSyntax::Lark, definition: "start: /.+/".into() }),
        custom_wire_name: Some("apply_patch".into()),
        ..Default::default()
    };
    for api in ["openai-completions", "openai-responses", "openai-codex-responses"] {
        for capability in [None, Some("function"), Some("freeform")] {
            let provider_id = if api == "openai-codex-responses" { "openai-codex" } else { "custom" };
            let base = if api == "openai-codex-responses" {
                "https://chatgpt.com/backend-api"
            } else {
                "https://custom.example/v1"
            };
            let mut donor = json!({"id":"fixture-donor","name":"Donor","api":api,"requestModelId":"fixture-wire-model","reasoning":false});
            if let Some(capability) = capability {
                donor["applyPatchToolType"] = json!(capability);
            }
            donor["provider"] = json!(provider_id);
            donor["baseUrl"] = json!(base);
            let reference = HostModel::new(Arc::new(VariantSpec::from_json(&donor)), HeaderSlot::Absent).unwrap();
            // Native custom-definition builders do not copy requestModelId or
            // applyPatchToolType. Supply the actual materialized startup donor
            // through its existing Host interface rather than inventing config.
            let mut provider = json!({"api":api,"baseUrl":base,
                "models":[{"id":"fixture-donor","name":"Donor","api":api,"reasoning":false}]});
            if api == "openai-codex-responses" {
                provider["auth"] = json!("oauth");
            } else {
                provider["apiKey"] = json!("FIXTURE_KEY");
            }
            let cfg = ModelsConfig::validate(json!({"providers":{provider_id:provider}})).unwrap();
            let cli = DailyOverrides {
                provider: Some(provider_id.into()),
                model: Some("fixture-donor".into()),
                ..Default::default()
            };
            let selected = resolve_daily_selection_with_reference(Some(&cfg), &cli, &no_env, &reference).unwrap();
            assert_eq!(selected.model.id, "fixture-wire-model", "wire model identity differs from capability donor");
            assert_eq!(selected.metadata["catalogModelId"], "fixture-donor");
            assert_eq!(selected.metadata.get("applyPatchToolType").and_then(Value::as_str), capability);
            let mut ctx = context();
            ctx.tools = Some(vec![edit.clone()]);
            let body = match &selected.protocol {
                ProtocolOptions::Completions(options) => {
                    let body = chat::build_params(&selected.model, &ctx, options).unwrap();
                    assert_eq!(body["tools"][0]["type"], "function");
                    assert_eq!(body["tools"][0]["function"]["name"], "edit");
                    assert!(body["tools"][0].get("format").is_none());
                    continue;
                }
                ProtocolOptions::Responses(options) => {
                    assert_eq!(options.request.apply_patch_tool_type.as_deref(), capability);
                    responses::build_request(&selected.model, &ctx, &options.request).unwrap()
                }
                ProtocolOptions::CodexResponses(options) => {
                    assert_eq!(options.request.apply_patch_tool_type.as_deref(), capability);
                    codex::build_request(&selected.model, &ctx, &options.request).unwrap()
                }
                ProtocolOptions::Anthropic(_) => panic!("selected daily OpenAI protocol"),
            };
            assert_eq!(body["model"], "fixture-wire-model");
            assert_eq!(body["tools"][0]["type"], if capability == Some("freeform") { "custom" } else { "function" });
            assert_eq!(body["tools"][0]["name"], if capability == Some("freeform") { "apply_patch" } else { "edit" });
        }
    }
}

fn no_strict_config(api: &str) -> (ModelsConfig, DailyOverrides) {
    let provider = if api == "openai-codex-responses" { "openai-codex" } else { "custom" };
    let base =
        if api == "openai-codex-responses" { "https://chatgpt.com/backend-api" } else { "https://fixture.example/v1" };
    let mut definition = json!({"api":api,"baseUrl":base,"models":[{"id":"schema-model","reasoning":false}]});
    definition["auth"] = json!(if api == "openai-codex-responses" { "oauth" } else { "none" });
    (
        ModelsConfig::validate(json!({"providers":{provider:definition}})).unwrap(),
        DailyOverrides { provider: Some(provider.into()), model: Some("schema-model".into()), ..Default::default() },
    )
}

// Projection checks for every protocol; only Chat's typed builder implements
// this schema policy. Actual Responses/Codex Graph HTTP observations live in
// model_route::codex_side_calls_preserve_cache_prefix_and_isolate_transport_state.
// Typed encoders do not consume this flag yet. This helper proves only the
// resolver projection; actual wire/schema effects are exercised separately by
// the existing Graph route, actual schema and real CLI coarse groups.
fn assert_no_strict_daily_projection(selection: &ara_cli::daily_model_config::DailySelection, expected: bool) {
    match &selection.protocol {
        ProtocolOptions::Completions(options) => assert_eq!(options.no_strict, expected),
        ProtocolOptions::Responses(options) => assert_eq!(options.request.no_strict, expected),
        ProtocolOptions::CodexResponses(options) => assert_eq!(options.request.no_strict, expected),
        _ => panic!("OpenAI schema policy"),
    }
}

fn observe_no_strict_daily_policy() {
    use ara_cli::schema_policy::{NO_STRICT_ENV, SchemaPolicySnapshot, parse_no_strict};
    for raw in ["1", "Y", "y", "TRUE", "true", "YES", "yes", "ON", "on"] {
        assert!(parse_no_strict(Some(raw)), "fixed truthy spelling {raw:?}");
    }
    for raw in [
        None,
        Some(""),
        Some("0"),
        Some("false"),
        Some("True"),
        Some("Yes"),
        Some("On"),
        Some(" true"),
        Some("true "),
        Some("1\n"),
    ] {
        assert!(!parse_no_strict(raw), "fixed non-truthy spelling {raw:?}");
    }
    for api in ["openai-completions", "openai-responses", "openai-codex-responses"] {
        let (cfg, cli) = no_strict_config(api);
        for (raw, expected) in [(None, false), (Some("true"), true), (Some("True"), false)] {
            let raw = std::sync::Mutex::new(raw.map(str::to_owned));
            let env = |name: &str| if name == NO_STRICT_ENV { raw.lock().unwrap().clone() } else { None };
            let snapshot = SchemaPolicySnapshot::capture(&env);
            assert_eq!(snapshot.no_strict(), expected);
            *raw.lock().unwrap() = Some(if expected { "false" } else { "true" }.into());
            let frozen = |name: &str| snapshot.environment_value(name, &env);
            assert_no_strict_daily_projection(&resolve_daily_selection(Some(&cfg), &cli, &frozen).unwrap(), expected);
        }
    }
    // Only Windows guarantees std::env mutation is safe in a multithreaded
    // process. Unix retains the injected-env observations above; no OS-env
    // lifetime claim is made there.
    #[cfg(windows)]
    for (raw, expected) in [(None, false), (Some("true"), true), (Some("True"), false), (Some("lowercase-key"), true)] {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--exact",
                "selected_catalog_freeform_capability_reaches_protocol_and_metadata_group",
                "--test-threads=1",
                "--nocapture",
            ])
            .env("ARA_SCHEMA_POLICY_CHILD", if expected { "true" } else { "false" })
            .env_remove(NO_STRICT_ENV);
        if let Some(raw) = raw {
            if raw == "lowercase-key" {
                child.env("ara_no_strict", "1");
            } else {
                child.env(NO_STRICT_ENV, raw);
            }
        }
        let receipt = child.output().unwrap();
        assert!(
            receipt.status.success(),
            "process snapshot child: {}\n{}",
            String::from_utf8_lossy(&receipt.stdout),
            String::from_utf8_lossy(&receipt.stderr)
        );
        let stderr = String::from_utf8_lossy(&receipt.stderr);
        let observations: Vec<_> =
            stderr.lines().filter(|line| line.starts_with("ARA-NO-STRICT PROCESS_SNAPSHOT PASS ")).collect();
        assert_eq!(observations.len(), 1, "child must reach its post-assertion receipt");
        eprintln!("{}", observations[0]);
    }
    #[cfg(windows)]
    eprintln!("ARA-NO-STRICT PROCESS_SNAPSHOT_CHILDREN PASS 4/4");
}

fn observe_process_schema_policy_child() -> bool {
    #[cfg(windows)]
    if let Ok(marker) = std::env::var("ARA_SCHEMA_POLICY_CHILD") {
        use ara_cli::schema_policy::{
            NO_STRICT_ENV, initialize_process_schema_policy, process_daily_environment, process_schema_policy,
        };
        let expected = marker == "true";
        let original = std::env::var(NO_STRICT_ENV).ok();
        initialize_process_schema_policy();
        assert_eq!(process_schema_policy().no_strict(), expected);
        // SAFETY: Rust documents set_var as always safe on Windows. This
        // synthetic mutation is confined to this exact-test child process.
        unsafe {
            std::env::set_var(NO_STRICT_ENV, if expected { "false" } else { "true" });
        }
        assert_eq!(std::env::var(NO_STRICT_ENV).unwrap(), if expected { "false" } else { "true" });
        assert_eq!(process_daily_environment(NO_STRICT_ENV), original);
        initialize_process_schema_policy();
        assert_eq!(process_schema_policy().no_strict(), expected);
        for api in ["openai-completions", "openai-responses", "openai-codex-responses"] {
            let (cfg, cli) = no_strict_config(api);
            for lifecycle in ["startup", "reload", "newSession"] {
                let selected = resolve_daily_selection(Some(&cfg), &cli, &process_daily_environment).unwrap();
                assert_no_strict_daily_projection(&selected, expected);
                let route = PreparedRoute::new(
                    selected.model,
                    selected.protocol,
                    Arc::new(FixedRequestAuth::new(ara_cli::model_route::RequestAuthLease::new(
                        CredentialIdentity::Runtime,
                        Some("fixture-key".into()),
                    ))),
                    0,
                )
                .unwrap();
                assert_eq!(route.model().api, api, "{lifecycle}");
            }
            let selector = ara_cli::retry_fallback::parse_retry_fallback_selector(
                &format!("{}/schema-model", cli.provider.as_ref().unwrap()),
                None,
            )
            .unwrap();
            assert_no_strict_daily_projection(
                &resolve_daily_fallback_selection(Some(&cfg), &selector, &process_daily_environment, None).unwrap(),
                expected,
            );
        }
        eprintln!(
            "ARA-NO-STRICT PROCESS_SNAPSHOT PASS raw={original:?} after={:?} frozen={:?} projected_no_strict={expected} protocols=3 lifetimes=4",
            std::env::var(NO_STRICT_ENV).ok(),
            process_daily_environment(NO_STRICT_ENV)
        );
        return true;
    }
    false
}

#[test]
fn file_loading_distinguishes_absence_validation_and_migration() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("models.yml");
    assert!(load_daily_config(&path, false).unwrap().is_none());
    assert!(load_daily_config(&path, true).is_err());
    std::fs::write(path.with_extension("yaml"), "providers:\n  local:\n    auth: none\n").unwrap();
    assert!(load_daily_config(&path, false).unwrap().is_some());
    std::fs::write(&path, "providers: {custom: {apiKey: secret-value, models: [{id: x}]}}").unwrap();
    let failure = load_daily_config(&path, false).err().unwrap().to_string();
    assert!(failure.contains("Validate(models)"));
    assert!(!failure.contains("secret-value"));
    std::fs::write(&path, "providers: [").unwrap();
    assert!(load_daily_config(&path, false).is_err());

    let migrated = directory.path().join("legacy.yml");
    let legacy = migrated.with_extension("json");
    std::fs::write(&legacy, "{/* retained */\"providers\":{\"local\":{\"auth\":\"none\"}},}").unwrap();
    assert!(load_daily_config(&migrated, false).unwrap().is_some());
    assert!(migrated.exists());
    assert!(std::fs::read_to_string(&legacy).unwrap().contains("retained"));
}

#[test]
fn configured_context_window_and_native_custom_defaults_reach_execution_route() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("models.json");
    for capacity in [None, Some(32_768.5)] {
        let mut provider = custom("openai-completions");
        provider["auth"] = json!("none");
        provider.as_object_mut().unwrap().remove("apiKey");
        if let Some(capacity) = capacity {
            provider["models"][0]["contextWindow"] = json!(capacity);
        }
        std::fs::write(&path, json!({"providers":{"custom":provider}}).to_string()).unwrap();
        let config = load_daily_config(&path, true).unwrap().unwrap();
        let selection = resolve_daily_selection(Some(&config), &selected(), &no_env).unwrap();
        // Fixed custom-models.ts::finalizeCustomModel(useDefaults:true) supplies
        // 128000 for a custom ID without a bundled reference or authored window.
        let expected_capacity = capacity.or(Some(128_000.0));
        assert_eq!(selection.model.context_window, expected_capacity);
        assert_eq!(selection.generation.max_tokens, Some(512));
        let ProtocolOptions::Completions(options) = &selection.protocol else { panic!("Chat protocol") };
        let body = chat::build_params(&selection.model, &context(), options).unwrap();
        // The fixed native builder supplies resolved compat for this unknown
        // custom route; its default field is max_completion_tokens.
        assert_eq!(options.compat.max_tokens_field, chat::MaxTokensField::MaxCompletionTokens);
        assert_eq!(body["max_completion_tokens"], 512);
        assert!(body.get("max_tokens").is_none());
        assert!(body.get("contextWindow").is_none());
        let DailyAuthSource::Fixed(lease) = selection.auth_source else { panic!("keyless lease") };
        let route =
            PreparedRoute::new(selection.model, selection.protocol, Arc::new(FixedRequestAuth::new(lease)), 0).unwrap();
        assert_eq!(route.model().context_window, expected_capacity);
    }
}

#[test]
fn catalog_tokenizers_and_explicit_host_overrides_preserve_the_wire_model_id() {
    use ara_ai::ModelTokenizer as T;
    for (id, family) in [
        ("deepseek-v4.1-flash", T::DeepSeekV3),
        ("qwen3.5-35b-a3b", T::Qwen3),
        ("kimi-k2.5", T::KimiK2),
        ("glm-5", T::Glm5),
        ("claude-opus-4.7", T::ClaudeV47),
    ] {
        let config = config(json!({"api":"openai-completions","baseUrl":"http://localhost/v1",
            "auth":"none","models":[{"id":id,"input":["text"]}]}));
        // Counting-family coverage uses the currently supported non-reasoning
        // Chat projection; provider reasoning transport has its own gate.
        let cli = DailyOverrides {
            provider: Some("custom".into()),
            model: Some(id.into()),
            reasoning: Some(false),
            ..Default::default()
        };
        let selection = resolve_daily_selection(Some(&config), &cli, &no_env).unwrap();
        assert_eq!(selection.model.id, id);
        assert_eq!(selection.model.tokenizer, Some(family), "{id}");
        let overridden = DailyOverrides { tokenizer: Some("none".into()), ..cli.clone() };
        assert_eq!(resolve_daily_selection(Some(&config), &overridden, &no_env).unwrap().model.tokenizer, None);
        let overridden = DailyOverrides { tokenizer: Some("kimi-k2".into()), ..cli };
        assert_eq!(
            resolve_daily_selection(Some(&config), &overridden, &no_env).unwrap().model.tokenizer,
            Some(T::KimiK2)
        );
    }
    for id in ["unclassified-alias", "qwen3-32b", "glm-4", "deepseek-r1-distill-qwen-32b"] {
        let cli = DailyOverrides {
            model: Some(id.into()),
            base_url: Some("http://localhost/v1".into()),
            ..Default::default()
        };
        let selection = resolve_daily_selection(None, &cli, &no_env).unwrap();
        assert_eq!(selection.model.tokenizer, None, "{id}");
        assert_eq!(selection.model.id, id);
    }
    let cli = DailyOverrides {
        model: Some("unclassified-alias".into()),
        base_url: Some("http://localhost/v1".into()),
        ..Default::default()
    };
    let selection =
        resolve_daily_selection(None, &cli, &|name: &str| (name == "ARA_TOKENIZER").then(|| "glm5".into())).unwrap();
    assert_eq!(selection.model.tokenizer, Some(T::Glm5));
    let cli = DailyOverrides { tokenizer: Some("none".into()), ..cli };
    assert_eq!(
        resolve_daily_selection(None, &cli, &|name: &str| (name == "ARA_TOKENIZER").then(|| "glm5".into()))
            .unwrap()
            .model
            .tokenizer,
        None
    );
}

#[test]
fn fallback_candidates_keep_their_own_contract_and_explicit_key_environment() {
    use ara_cli::retry_fallback::parse_retry_fallback_selector;
    let baseline = json!({"providers":{
        "target":{"api":"openai-responses","baseUrl":"https://target.example/v1","apiKey":"TARGET_KEY",
            "models":[{"id":"group/large","contextWindow":200,"maxTokens":512,"input":["text"]}]}
    }});
    let env = |name: &str| match name {
        "ARA_MODEL" | "ARA_TEST_MODEL_ID" => Some("old-model".into()),
        "ARA_PROVIDER" => Some("old-provider".into()),
        "ARA_BASE_URL" | "ARA_TEST_BASE_URL" | "OPENROUTER_BASE_URL" => Some("https://old.example/v1".into()),
        "ARA_TOKENIZER" => Some("unsupported-old-tokenizer".into()),
        "ARA_API_KEY" | "ARA_TEST_API_KEY" => Some("old-ambient-key".into()),
        "TARGET_KEY" => Some("target-key".into()),
        _ => None,
    };
    let selector = parse_retry_fallback_selector("target/group/large", None).unwrap();
    for key_name in ["TARGET_KEY", "ARA_API_KEY", "ARA_BASE_URL", "literal-target-key"] {
        let mut value = baseline.clone();
        value["providers"]["target"]["apiKey"] = json!(key_name);
        let cfg = ModelsConfig::validate(value).unwrap();
        let target = resolve_daily_fallback_selection(Some(&cfg), &selector, &env, None).unwrap();
        assert_eq!(target.model.provider, "target");
        assert_eq!(target.model.id, "group/large");
        assert_eq!(target.model.base_url, "https://target.example/v1");
        assert_eq!(target.api, DailyApi::OpenAiResponses);
        assert_eq!(target.model.context_window, Some(200.0));
        assert_eq!(target.generation.max_tokens, Some(512));
        let ProtocolOptions::Responses(options) = &target.protocol else { panic!("candidate Responses contract") };
        let body = responses::build_request(&target.model, &Context::default(), &options.request).unwrap();
        assert_eq!(body["model"], "group/large");
        let DailyAuthSource::Fixed(lease) = target.auth_source else { panic!("candidate's own key") };
        assert_eq!(
            lease.identity(),
            &if key_name == "literal-target-key" {
                CredentialIdentity::Config { provider: "target".into() }
            } else {
                CredentialIdentity::Environment { variable: key_name.into() }
            }
        );
        assert!(PreparedRoute::new(target.model, target.protocol, Arc::new(FixedRequestAuth::new(lease)), 1).is_ok());
    }
    let cfg = ModelsConfig::validate(baseline).unwrap();
    for raw in ["target/missing", "absent/group/large"] {
        let missing = parse_retry_fallback_selector(raw, None).unwrap();
        assert!(resolve_daily_fallback_selection(Some(&cfg), &missing, &env, None).is_err());
    }
    let mut value = cfg.value().clone();
    value["providers"]["target"]["api"] = json!("anthropic-messages");
    let unsupported = ModelsConfig::validate(value).unwrap();
    assert!(resolve_daily_fallback_selection(Some(&unsupported), &selector, &env, None).is_err());
}

#[test]
fn context_promotion_uses_target_contract_and_explicit_auth_without_old_route_overrides() {
    let baseline = json!({"providers":{
        "source":{"api":"openai-responses","baseUrl":"https://source.example/v1","auth":"none",
            "models":[{"id":"small","contextWindow":100,"input":["text"],"contextPromotionTarget":"target/group/large"}]},
        "target":{"api":"openai-completions","baseUrl":"https://target.example/v1","apiKey":"TARGET_KEY",
            "models":[{"id":"group/large","contextWindow":200,"maxTokens":512,"input":["text"]}]}
    }});
    let config = ModelsConfig::validate(baseline.clone()).unwrap();
    let env = |name: &str| match name {
        "ARA_MODEL" | "ARA_TEST_MODEL_ID" => Some("small".into()),
        "ARA_PROVIDER" => Some("source".into()),
        "ARA_BASE_URL" | "ARA_TEST_BASE_URL" | "OPENROUTER_BASE_URL" => Some("https://old.example/v1".into()),
        "ARA_TOKENIZER" => Some("unsupported-old-tokenizer".into()),
        "ARA_API_KEY" => Some("explicit-same-name-target-key".into()),
        "ARA_TEST_API_KEY" => Some("old-ambient-key".into()),
        "TARGET_KEY" => Some("target-key".into()),
        _ => None,
    };
    let current = resolve_daily_selection(
        Some(&config),
        &DailyOverrides {
            provider: Some("source".into()),
            model: Some("small".into()),
            tokenizer: Some("none".into()),
            ..Default::default()
        },
        &env,
    )
    .unwrap();
    assert_eq!(
        current.metadata.as_object().unwrap().keys().map(String::as_str).collect::<std::collections::BTreeSet<_>>(),
        [
            "api",
            "catalogModelId",
            "compat",
            "contextPromotionTarget",
            "contextWindow",
            "id",
            "input",
            "provider",
            "reasoning",
            "supportsComputerUse"
        ]
        .into_iter()
        .collect()
    );
    assert_eq!(current.metadata["catalogModelId"], "small");
    assert_eq!(current.metadata["supportsComputerUse"], false);
    assert_eq!(current.metadata["reasoning"], false);
    assert_eq!(current.metadata["input"], json!(["text"]));
    assert_eq!(current.metadata["contextWindow"].as_f64(), Some(100.0));
    assert_eq!(current.metadata["contextPromotionTarget"], "target/group/large");
    let promoted = resolve_daily_promotion_selection(Some(&config), &current.metadata, &env).unwrap().unwrap();
    assert_eq!(promoted.model.provider, "target");
    assert_eq!(promoted.model.id, "group/large");
    assert_eq!(promoted.model.base_url, "https://target.example/v1");
    assert_eq!(promoted.model.context_window, Some(200.0));
    assert_eq!(promoted.metadata["input"], json!(["text"]));
    assert_eq!(promoted.api, DailyApi::OpenAiCompletions);
    assert_eq!(promoted.generation.max_tokens, Some(512));
    let DailyAuthSource::Fixed(lease) = promoted.auth_source else { panic!("target key lease") };
    assert_eq!(lease.identity(), &CredentialIdentity::Environment { variable: "TARGET_KEY".into() });
    assert!(PreparedRoute::new(promoted.model, promoted.protocol, Arc::new(FixedRequestAuth::new(lease)), 1).is_ok());

    for key_name in ["TARGET_KEY", "ARA_API_KEY", "ARA_BASE_URL"] {
        let mut value = baseline.clone();
        value["providers"]["target"]["apiKey"] = json!(key_name);
        let cfg = ModelsConfig::validate(value).unwrap();
        let target = resolve_daily_promotion_selection(Some(&cfg), &current.metadata, &env).unwrap().unwrap();
        assert_eq!(target.model.base_url, "https://target.example/v1", "explicit key names cannot override endpoints");
        let DailyAuthSource::Fixed(lease) = target.auth_source else { panic!("explicit target key") };
        assert_eq!(lease.identity(), &CredentialIdentity::Environment { variable: key_name.into() });
        if key_name != "TARGET_KEY" {
            assert!(resolve_daily_promotion_selection(Some(&cfg), &current.metadata, &no_env).is_err());
        }
    }
    for auth_none in [true, false] {
        let mut value = baseline.clone();
        if auth_none {
            value["providers"]["target"].as_object_mut().unwrap().remove("apiKey");
            value["providers"]["target"]["auth"] = json!("none");
        } else {
            value["providers"]["target"]["apiKey"] = json!("ARA_TEST_API_KEY");
        }
        let cfg = ModelsConfig::validate(value).unwrap();
        let missing_target_env = |name: &str| {
            if name == "ARA_TEST_API_KEY" { None } else { env(name) }
        };
        assert_eq!(
            resolve_daily_promotion_selection(Some(&cfg), &current.metadata, &missing_target_env).is_ok(),
            auth_none
        );
    }
    let mut value = baseline.clone();
    value["providers"]["source"]["models"][0].as_object_mut().unwrap().remove("contextPromotionTarget");
    value["providers"]["source"]["modelOverrides"] = json!({"small":{"contextPromotionTarget":"target/group/large"}});
    let cfg = ModelsConfig::validate(value.clone()).unwrap();
    let cli = DailyOverrides { provider: Some("source".into()), model: Some("small".into()), ..Default::default() };
    assert_eq!(
        resolve_daily_selection(Some(&cfg), &cli, &no_env).unwrap().metadata["contextPromotionTarget"],
        "target/group/large"
    );
    value["providers"]["source"]["modelOverrides"]["small"]["reasoning"] = json!(true);
    let cfg = ModelsConfig::validate(value).unwrap();
    // The complete native patch now supports reasoning overrides. Responses
    // can execute this setting; the earlier narrow projection rejected it.
    let patched = resolve_daily_selection(Some(&cfg), &cli, &no_env).unwrap();
    assert_eq!(patched.api, DailyApi::OpenAiResponses);
    assert!(patched.model.reasoning);

    let mut bundle_target = current.metadata.clone();
    bundle_target["contextPromotionTarget"] = json!("openai-codex/gpt-5.4");
    assert!(resolve_daily_promotion_selection(Some(&config), &bundle_target, &no_env).is_err());
    let mut value = baseline;
    value["providers"]["target"]["models"][0]["preferWebsockets"] = json!(true);
    let cfg = ModelsConfig::validate(value).unwrap();
    assert!(resolve_daily_promotion_selection(Some(&cfg), &current.metadata, &env).is_err());
}

#[test]
fn chat_selection_precedence_reaches_encoder_and_keeps_auth_private() {
    let mut value = custom("openai-completions");
    value["compat"] =
        json!({"supportsDeveloperRole":true,"supportsUsageInStreaming":false,"maxTokensField":"max_completion_tokens"});
    value["headers"] = json!({"X-Route":"provider","Authorization":"Bearer fixture"});
    value["models"][0]["baseUrl"] = json!("https://model.example/v1");
    value["models"][0]["headers"] = json!({"x-route":"model"});
    value["models"][0]["compat"] =
        json!({"supportsUsageInStreaming":true,"streamIdleTimeoutMs":1250,"thinkingLoopGuard":false});
    let config = config(value);
    let env = |name: &str| match name {
        "ARA_PROVIDER" => Some("not-selected".into()),
        "ARA_BASE_URL" => Some("https://env.example/v1".into()),
        "ARA_MODEL" => Some("not-selected".into()),
        "ARA_API_KEY" => Some("ambient-fixture".into()),
        "EXPLICIT_KEY" => Some("explicit-fixture".into()),
        _ => None,
    };
    let mut cli = selected();
    let intermediate = resolve_daily_selection(Some(&config), &cli, &env).unwrap();
    assert_eq!(intermediate.model.base_url, "https://env.example/v1");
    let env_selection = |name: &str| match name {
        "ARA_PROVIDER" => Some("custom".into()),
        "ARA_MODEL" => Some("group/exact-model".into()),
        _ => None,
    };
    assert_eq!(
        resolve_daily_selection(Some(&config), &DailyOverrides::default(), &env_selection).unwrap().model.provider,
        "custom"
    );
    cli.base_url = Some("https://cli.example/v1".into());
    cli.api_key_env = Some("EXPLICIT_KEY".into());
    cli.max_tokens = Some(64);
    cli.temperature = Some(0.25);
    cli.headers = vec![("X-Route".into(), "cli".into())];
    let route = resolve_daily_selection(Some(&config), &cli, &env).unwrap();
    // Native semantic guard checks property presence, including authored false;
    // this is host metadata and must not enter the wire request.
    assert!(route.loop_guard_policy.semantic_heuristics);
    assert_eq!(route.model.id, "group/exact-model");
    assert_eq!(route.model.base_url, "https://cli.example/v1");
    assert_eq!(route.generation.max_tokens, Some(64));
    let ProtocolOptions::Completions(options) = &route.protocol else { panic!("Chat protocol") };
    assert!(options.api_key.is_none());
    // Configured headers now materialize into the private per-request lease;
    // actual CLI wire tests check provider/model/override/CLI precedence.
    assert!(options.extra_headers.is_empty());
    assert_eq!(options.idle_timeout, Some(Duration::from_millis(1250)));
    // supportsDeveloperRole governs explicit developer messages. A system
    // prompt on this non-reasoning model remains a system message upstream.
    let mut input = context();
    input.system_prompt.clear();
    input.messages.insert(
        0,
        Message::Developer(DeveloperMessage {
            content: UserContent::Text("Be concise".into()),
            timestamp: 0,
            provider_payload: None,
            provider_payload_origin: None,
        }),
    );
    let body = chat::build_params(&route.model, &input, options).unwrap();
    assert_eq!(body["max_completion_tokens"], 64);
    assert!(body.get("max_tokens").is_none());
    assert_eq!(body["temperature"], 0.25);
    assert_eq!(body["messages"][0]["role"], "developer");
    assert_eq!(body["stream_options"]["include_usage"], true);
    assert!(body.get("thinkingLoopGuard").is_none());
    let DailyAuthSource::Configured(spec) = route.auth_source else { panic!("configured private lease") };
    assert_eq!(spec.base.identity(), &CredentialIdentity::Environment { variable: "EXPLICIT_KEY".into() });
    let auth = ara_cli::config_request_auth::ConfigRequestAuth::new(spec, std::env::current_dir().unwrap(), None);
    assert!(PreparedRoute::new(route.model, route.protocol, Arc::new(auth), 0).is_ok());

    // The explicit Host mode must survive both authored base and whenThinking
    // compat. Exercise its Native wire consumer, retaining unset/false defaults.
    let mut value = custom("openai-completions");
    value["models"][0]["reasoning"] = json!(true);
    value["models"][0]["thinking"] = json!({"mode":"effort","efforts":["high"]});
    let replay_config = ModelsConfig::validate(json!({"providers":{"custom":value}})).unwrap();
    let donor = json!({
        "id":"group/exact-model","api":"openai-completions","provider":"custom",
        "baseUrl":"https://config.example/v1","reasoning":true,"supportsTools":true,"input":["text"],
        "thinking":{"mode":"effort","efforts":["high"]},"compat":{
        "requiresReasoningContentForAllAssistantTurns":false,
        "requiresReasoningContentForToolCalls":false,
        "allowsSyntheticReasoningContentForToolCalls":true,
        "whenThinking":{
            "requiresReasoningContentForAllAssistantTurns":false,
            "requiresReasoningContentForToolCalls":false,
            "allowsSyntheticReasoningContentForToolCalls":true
        }}
    });
    let reference = ara_cli::model_patch::HostModel::new(
        Arc::new(ara_cli::model_collapse::VariantSpec::from_json(&donor)),
        ara_cli::model_patch::HeaderSlot::Absent,
    )
    .unwrap();
    for explicit in [None, Some(false), Some(true)] {
        let mut cli = selected();
        cli.chat_replay_reasoning_content = explicit;
        let route = ara_cli::daily_model_config::resolve_daily_selection_with_reference(
            Some(&replay_config),
            &cli,
            &no_env,
            &reference,
        )
        .unwrap();
        let ProtocolOptions::Completions(mut options) = route.protocol else { panic!("Native Chat mode") };
        options.reasoning =
            Some(ara_ai::ReasoningRequest { effort: Some(ara_ai::ReasoningEffort::High), ..Default::default() });
        let factory = Arc::new(
            ara_ai::graph_values::GraphValueFactory::new(
                ara_rpc::value_heap::ValueHeap::new().unwrap(),
                Arc::new(ara_rpc::value_heap::NoRuntime),
            )
            .unwrap(),
        );
        let messages = json!([{"role":"assistant","provider":"custom","api":"openai-completions",
            "model":"group/exact-model","timestamp":1,"stopReason":"stop","content":[
                {"type":"thinking","thinking":"retained thought","thinkingSignature":"reasoning_content"},
                {"type":"text","text":"retained answer"}
            ]},{"role":"assistant","provider":"custom","api":"openai-completions",
            "model":"group/exact-model","timestamp":2,"stopReason":"stop",
            "content":[{"type":"text","text":"text-only answer"}]}]);
        let messages = factory.heap().import_strict(ara_rpc::WireValue::parse(&messages.to_string()).unwrap()).unwrap();
        let original_context = factory.heap().object().unwrap();
        for (key, value) in [
            ("messages", messages),
            ("systemPrompt", factory.heap().array(vec![]).unwrap()),
            ("tools", factory.heap().undefined().unwrap()),
        ] {
            factory.heap().define_own_data(&original_context, key.into(), value, factory.runtime()).unwrap();
        }
        let original = ara_ai::GraphProviderContext {
            original_context: Some(original_context),
            system_prompt: vec![],
            messages: vec![],
            tools: None,
            history_demotion_dialect: None,
            history_id_hash: None,
            history_id_hash_function: None,
        };
        let wire = chat::actual::build_request(&route.model, &original, &options, factory).unwrap();
        let body: Value = serde_json::from_slice(&wire).unwrap();
        assert_eq!(body["messages"][0]["content"], "retained answer");
        assert_eq!(body["messages"][1]["content"], "text-only answer");
        if explicit == Some(true) {
            assert_eq!(body["messages"][0]["reasoning_content"], "retained thought");
            assert_eq!(body["messages"][1]["reasoning_content"], "");
        } else {
            assert!(body["messages"][0].get("reasoning_content").is_none(), "{body:#?}");
            assert!(body["messages"][1].get("reasoning_content").is_none(), "{body:#?}");
        }
    }
}

#[test]
fn responses_and_keyless_config_are_not_reduced_to_chat_defaults() {
    let mut value = custom("openai-completions");
    value["auth"] = json!("none");
    value.as_object_mut().unwrap().remove("apiKey");
    value["models"][0]["api"] = json!("openai-responses");
    value["models"][0]["reasoning"] = json!(true);
    let config = config(value);
    let mut cli = selected();
    cli.stream_idle_timeout = Some(0.0);
    cli.responses_stateful = Some(true);
    let env = |name: &str| (name == "ARA_API_KEY").then(|| "must-not-be-used".into());
    let route = resolve_daily_selection(Some(&config), &cli, &env).unwrap();
    assert_eq!(route.api, DailyApi::OpenAiResponses);
    let ProtocolOptions::Responses(options) = &route.protocol else { panic!("Responses protocol") };
    assert!(options.api_key.is_none());
    assert!(options.stateful_responses);
    assert_eq!(options.idle_timeout, None);
    let body = responses::build_request(&route.model, &context(), &options.request).unwrap();
    assert_eq!(body["max_output_tokens"], 512);
    assert!(body["include"].as_array().unwrap().contains(&json!("reasoning.encrypted_content")));
    let DailyAuthSource::Fixed(lease) = route.auth_source else { panic!("keyless lease") };
    assert_eq!(lease.identity(), &CredentialIdentity::Keyless);

    cli.api = Some("openai-completions".into());
    cli.reasoning = Some(false);
    cli.responses_stateful = Some(false);
    assert_eq!(resolve_daily_selection(Some(&config), &cli, &env).unwrap().api, DailyApi::OpenAiCompletions);
}

#[test]
fn thinking_reaches_encoders_and_unsupported_selected_semantics_fail_explicitly() {
    let cli = selected();
    for api in ["openai-completions", "openai-responses"] {
        let mut provider = custom(api);
        provider["models"][0]["reasoning"] = json!(true);
        provider["models"][0]["thinking"] = json!({"mode":"effort","efforts":["high"]});
        let cfg = config(provider);
        let route = resolve_daily_selection(Some(&cfg), &cli, &no_env).unwrap();
        let request = ara_ai::ReasoningRequest { effort: Some(ara_ai::ReasoningEffort::High), ..Default::default() };
        match route.protocol {
            ProtocolOptions::Completions(mut options) => {
                options.reasoning = Some(request);
                let body = chat::build_params(&route.model, &context(), &options).unwrap();
                assert_eq!(body["reasoning_effort"], "high");
            }
            ProtocolOptions::Responses(mut options) => {
                options.request.reasoning = Some(request);
                let body = responses::build_request(&route.model, &context(), &options.request).unwrap();
                assert_eq!(body["reasoning"]["effort"], "high");
            }
            _ => panic!("selected OpenAI protocol"),
        }
    }
    for (key, value) in [("preferWebsockets", json!(true)), ("omitMaxOutputTokens", json!(true))] {
        let mut provider = custom("openai-responses");
        provider["models"][0][key] = value;
        let cfg = config(provider);
        assert!(resolve_daily_selection(Some(&cfg), &cli, &no_env).err().unwrap().field.contains(key));
    }
    let mut provider = custom("openai-completions");
    provider["compat"] = json!({"extraBody":{"secret-field":"secret-value"}});
    let cfg = config(provider);
    let failure = resolve_daily_selection(Some(&cfg), &cli, &no_env).err().unwrap().to_string();
    assert!(failure.contains("compat/extraBody"));
    assert!(!failure.contains("secret-value"));
    let mut provider = custom("openai-completions");
    provider["headers"] = json!({"bad name":"!echo private-command"});
    let cfg = config(provider);
    let failure = resolve_daily_selection(Some(&cfg), &cli, &no_env).err().unwrap().to_string();
    assert!(failure.contains("headers"));
    assert!(!failure.contains("private-command"));
    let cfg = config(custom("openai-responses"));
    let missing_provider = DailyOverrides { provider: None, ..selected() };
    assert!(resolve_daily_selection(Some(&cfg), &missing_provider, &no_env).is_err());
    let missing_model = DailyOverrides { model: Some("other".into()), ..selected() };
    assert!(resolve_daily_selection(Some(&cfg), &missing_model, &no_env).is_err());
}

#[test]
fn codex_account_route_needs_no_config_or_key_and_rejects_overrides() {
    let cli = DailyOverrides {
        provider: Some("openai-codex".into()),
        model: Some("gpt-5.3-codex".into()),
        ..Default::default()
    };
    let env = |name: &str| (name == "ARA_API_KEY").then(|| "custom-key-must-not-be-used".into());
    let route = resolve_daily_selection(None, &cli, &env).unwrap();
    assert_eq!(route.api, DailyApi::OpenAiCodexResponses);
    assert_eq!(route.model.base_url, "https://chatgpt.com/backend-api");
    assert_eq!(route.model.context_window, None);
    assert!(matches!(route.auth_source, DailyAuthSource::OpenAiCodex));
    let ProtocolOptions::CodexResponses(options) = route.protocol else { panic!("Codex protocol") };
    assert!(options.api_key.is_none());
    assert!(options.session_id.is_none());
    // An existing custom configuration must not disable the built-in account route.
    let custom_config = config(custom("openai-completions"));
    assert!(matches!(
        resolve_daily_selection(Some(&custom_config), &cli, &env).unwrap().auth_source,
        DailyAuthSource::OpenAiCodex
    ));
    let local = DailyOverrides { base_url: Some("http://127.0.0.1:12345/backend-api".into()), ..cli.clone() };
    assert_eq!(resolve_daily_selection(None, &local, &env).is_ok(), cfg!(feature = "test-fixture"));
    for invalid in [
        DailyOverrides { max_tokens: Some(20), ..cli.clone() },
        DailyOverrides { temperature: Some(0.2), ..cli.clone() },
        DailyOverrides { base_url: Some("https://custom.example".into()), ..cli.clone() },
        DailyOverrides { base_url: Some("http://127.0.0.1:12345/backend-api?route=other".into()), ..cli.clone() },
        DailyOverrides { responses_stateful: Some(true), ..cli.clone() },
        DailyOverrides { headers: vec![("ChatGPT-Account-ID".into(), "wrong".into())], ..cli.clone() },
        DailyOverrides { api_key_env: Some("ARA_API_KEY".into()), ..cli.clone() },
    ] {
        assert!(resolve_daily_selection(None, &invalid, &env).is_err());
    }
}

#[test]
fn selected_computer_capability_and_images_follow_materialized_donor_group() {
    use ara_ai::providers::openai_codex_responses as codex;
    use ara_ai::{NativeToolMarker, Tool};
    use ara_cli::daily_model_config::resolve_daily_selection_with_reference;
    use ara_cli::model_collapse::VariantSpec;
    use ara_cli::model_patch::{HeaderSlot, HostModel};

    for api in ["openai-responses", "openai-codex-responses"] {
        let provider_id = if api == "openai-codex-responses" { "openai-codex" } else { "custom" };
        let base = if api == "openai-codex-responses" {
            "https://chatgpt.com/backend-api"
        } else {
            "https://custom.example/v1"
        };
        for capability in [None, Some(Value::Null), Some(json!(false)), Some(json!(true))] {
            let mut donor = json!({"id":"computer-donor","requestModelId":"computer-wire-id",
                "provider":provider_id,"api":api,"baseUrl":base,"reasoning":false,"input":["text","image"]});
            if let Some(value) = &capability {
                donor["supportsComputerUse"] = value.clone();
            }
            let reference = HostModel::new(Arc::new(VariantSpec::from_json(&donor)), HeaderSlot::Absent).unwrap();
            let mut provider = json!({"api":api,"baseUrl":base,
                "models":[{"id":"computer-donor","api":api,"reasoning":false,"input":["text"]}]});
            if api == "openai-codex-responses" {
                provider["auth"] = json!("oauth");
            } else {
                provider["apiKey"] = json!("FIXTURE_KEY");
            }
            let cfg = ModelsConfig::validate(json!({"providers":{provider_id:provider}})).unwrap();
            let cli = DailyOverrides {
                provider: Some(provider_id.into()),
                model: Some("computer-donor".into()),
                ..Default::default()
            };
            let selection = resolve_daily_selection_with_reference(Some(&cfg), &cli, &no_env, &reference).unwrap();
            let supported = capability == Some(json!(true));
            assert_eq!(selection.metadata.get("supportsComputerUse"), capability.as_ref());
            assert_eq!(selection.metadata["input"], json!(["text", "image"]));
            assert_eq!(selection.model.id, "computer-wire-id");
            let mut ctx = context();
            ctx.tools = Some(vec![Tool {
                name: "computer".into(),
                description: "generic provider-native fixture".into(),
                parameters: json!({"type":"object","properties":{}}),
                native: Some(NativeToolMarker::Computer),
                ..Default::default()
            }]);
            ctx.messages.push(Message::User(UserMessage {
                content: UserContent::Blocks(vec![ara_ai::UserBlock::Image(ara_ai::ImageContent {
                    data: "aW1hZ2U=".into(),
                    mime_type: "image/png".into(),
                    detail: None,
                    compaction_frame: false,
                })]),
                synthetic: None,
                timestamp: 0,
                provider_payload: None,
                provider_payload_origin: None,
            }));
            let body = match &selection.protocol {
                ProtocolOptions::Responses(options) => {
                    assert_eq!(options.request.supports_computer_use, supported);
                    assert!(options.request.supports_images);
                    responses::build_request(&selection.model, &ctx, &options.request).unwrap()
                }
                ProtocolOptions::CodexResponses(options) => {
                    assert_eq!(options.request.supports_computer_use, supported);
                    assert!(options.request.supports_images);
                    codex::build_request(&selection.model, &ctx, &options.request).unwrap()
                }
                _ => panic!("selected Responses family"),
            };
            assert_eq!(body["tools"][0]["type"], if supported { "computer" } else { "function" });
            assert_eq!(body["model"], "computer-wire-id");
            assert!(
                body["input"].as_array().unwrap().iter().any(|item| {
                    item.get("content")
                        .and_then(Value::as_array)
                        .is_some_and(|content| content.iter().any(|part| part["type"] == "input_image"))
                }),
                "selected donor's image input must reach the actual encoder"
            );
        }
    }
}

// Host decoding policy consumes daily metadata, independently of inference
// wire options. These groups connect configuration/catalog/restore selectors
// to the actual ReadHostSnapshot consumer. Full RPC publication and process
// resume remain separate Host acceptance, not implied by this native group.
fn assert_read_decoder_contract(selection: &ara_cli::daily_model_config::DailySelection, expected: Option<&str>) {
    use ara_cli::host_browser::ReadHostSnapshot;
    assert_eq!(selection.metadata.get("imageInputDecoder").and_then(Value::as_str), expected);
    let snapshot = ReadHostSnapshot::for_model(&selection.model, Some(&selection.metadata), false, None);
    assert_eq!(snapshot.image_input_decoder.as_deref(), expected);
    assert_eq!(snapshot.current_runtime().model_excludes_webp, expected == Some("stb"));
    let body = match &selection.protocol {
        ProtocolOptions::Completions(options) => chat::build_params(&selection.model, &context(), options).unwrap(),
        ProtocolOptions::Responses(options) => {
            responses::build_request(&selection.model, &context(), &options.request).unwrap()
        }
        _ => panic!("fixture uses ordinary OpenAI adapters"),
    };
    assert_eq!(body["model"], selection.model.id);
    assert!(!body.to_string().contains("imageInputDecoder"), "decoder is Host policy, not provider wire configuration");
}

fn read_decoder_authored_override_and_raw_daily_paths_group() {
    let temporary = tempfile::tempdir().unwrap();
    let file = temporary.path().join("models.json");
    for api in ["openai-completions", "openai-responses"] {
        for source in ["authored", "override", "normal"] {
            let mut provider = json!({"baseUrl":"https://decoder.test.invalid/v1","auth":"none",
                "models":[{"id":"group/exact-model","reasoning":false,"input":["text","image"]}]});
            provider["api"] = json!(api);
            match source {
                "authored" => provider["models"][0]["imageInputDecoder"] = json!("stb"),
                "override" => provider["modelOverrides"] = json!({"group/exact-model":{"imageInputDecoder":"stb"}}),
                "normal" => {}
                _ => unreachable!(),
            }
            std::fs::write(&file, json!({"providers":{"custom":provider}}).to_string()).unwrap();
            let cfg = load_daily_config(&file, true).unwrap().unwrap();
            let selection = resolve_daily_selection(Some(&cfg), &selected(), &no_env).unwrap();
            assert!(selection.catalog_model.is_some());
            assert_read_decoder_contract(&selection, (source != "normal").then_some("stb"));
        }
    }
    // A legal provider-only route can select a raw ID absent from the catalog.
    // Its decoder override follows the same existing daily fallback projection.
    for decoder in [Some("stb"), None] {
        let mut provider =
            json!({"api":"openai-completions","baseUrl":"https://decoder.test.invalid/v1","auth":"none"});
        if let Some(decoder) = decoder {
            provider["modelOverrides"] = json!({"group/exact-model":{"imageInputDecoder":decoder}});
        }
        let cfg = config(provider);
        let selection = resolve_daily_selection(Some(&cfg), &selected(), &no_env).unwrap();
        assert!(selection.catalog_model.is_none());
        assert_read_decoder_contract(&selection, decoder);
    }
    // The existing schema permits only stb, not a new decoder vocabulary.
    for value in [json!("other"), Value::Null, json!(false)] {
        for source in ["models", "modelOverrides"] {
            let mut provider = json!({"api":"openai-completions","baseUrl":"https://decoder.test.invalid/v1","auth":"none",
                "models":[{"id":"group/exact-model"}]});
            if source == "models" {
                provider["models"][0]["imageInputDecoder"] = value.clone();
            } else {
                provider["modelOverrides"] = json!({"group/exact-model":{"imageInputDecoder":value}});
            }
            assert!(ModelsConfig::validate(json!({"providers":{"custom":provider}})).is_err());
        }
    }
}

fn read_decoder_materialized_donor_restore_and_role_selection_group() {
    use ara_cli::daily_model_config::{resolve_daily_role_selection, resolve_daily_selection_with_reference};
    use ara_cli::model_collapse::VariantSpec;
    use ara_cli::model_patch::{HeaderSlot, HostModel};
    use ara_cli::retry_fallback::parse_retry_fallback_selector;
    use ara_cli::startup_model::{parse_saved_model, pick_default_available_model};
    use ara_cli::static_model_registry::{StaticModelRegistry, StaticRegistryInputs};
    let temporary = tempfile::tempdir().unwrap();
    let mut journal = ara_session::SessionJournal::create(temporary.path(), temporary.path()).unwrap();
    journal.append_model_change("custom/group/exact-model").unwrap();
    journal.materialize().unwrap();
    let reopened = ara_session::SessionJournal::open(journal.path()).unwrap();
    let candidates = reopened.restorable_model_candidates().unwrap();
    assert_eq!(candidates, vec!["custom/group/exact-model"]);
    let cfg = config(json!({"api":"openai-completions","baseUrl":"https://decoder.test.invalid/v1","auth":"none",
        "modelOverrides":{"group/exact-model":{"imageInputDecoder":"stb"}}}));
    // Runtime/catalog donors can carry metadata independent of the narrower
    // authored schema. The actual model patch overrides this older donor.
    let donor = HostModel::new(
        Arc::new(VariantSpec::from_json(&json!({
        "id":"group/exact-model","name":"Decoder donor","requestModelId":"decoder-wire-id","provider":"custom",
        "api":"openai-completions","baseUrl":"https://decoder.test.invalid/v1","reasoning":false,
        "input":["text","image"],"supportsTools":true,"contextWindow":128000,"maxTokens":512,
        "imageInputDecoder":"legacy-runtime-decoder"}))),
        HeaderSlot::Absent,
    )
    .unwrap();
    let registry = StaticModelRegistry::from_inputs(
        Some(&cfg),
        StaticRegistryInputs { bundled_models: vec![donor], ..Default::default() },
    )
    .unwrap();
    let parsed = parse_saved_model(&candidates[0], &|provider, id| {
        registry.find_exact(&provider.into(), &id.into()).unwrap().is_some()
    })
    .unwrap();
    let target = registry.find_exact(&parsed.provider.as_str().into(), &parsed.id.as_str().into()).unwrap().unwrap();
    assert!(
        target
            .spec()
            .get("imageInputDecoder")
            .and_then(ara_rpc::WireValue::as_string)
            .is_some_and(|value| value.equals_ascii("stb"))
    );
    let startup = pick_default_available_model(&[target.clone()], &|_| true).unwrap();
    assert!(Arc::ptr_eq(&startup, &target));
    let selection = resolve_daily_selection_with_reference(Some(&cfg), &selected(), &no_env, &startup).unwrap();
    assert_eq!(selection.model.id, "decoder-wire-id");
    assert_read_decoder_contract(&selection, Some("stb"));
    let selector = parse_retry_fallback_selector(&candidates[0], None).unwrap();
    let selected_role = resolve_daily_role_selection(Some(&cfg), &selector, &no_env, &target).unwrap();
    assert_read_decoder_contract(&selected_role, Some("stb"));
    // Authored stb must not override a separately materialized normal donor.
    // This is the donor owned route used by startup and native model selection.
    let raw_stb = config(json!({"api":"openai-completions","baseUrl":"https://decoder.test.invalid/v1","auth":"none",
        "models":[{"id":"group/exact-model","imageInputDecoder":"stb"}]}));
    for decoder in [None, Some("stb"), None] {
        let mut spec = json!({"id":"group/exact-model","requestModelId":"decoder-wire-id","provider":"custom",
            "api":"openai-completions","baseUrl":"https://decoder.test.invalid/v1","reasoning":false,
            "input":["text","image"],"supportsTools":true});
        if let Some(decoder) = decoder {
            spec["imageInputDecoder"] = json!(decoder);
        }
        let donor = HostModel::new(Arc::new(VariantSpec::from_json(&spec)), HeaderSlot::Absent).unwrap();
        let selected = resolve_daily_selection_with_reference(Some(&raw_stb), &selected(), &no_env, &donor).unwrap();
        assert_read_decoder_contract(&selected, decoder);
        let role = resolve_daily_role_selection(Some(&raw_stb), &selector, &no_env, &donor).unwrap();
        assert_read_decoder_contract(&role, decoder);
    }
}

fn read_decoder_keeps_unimplemented_wire_options_rejected_group() {
    for field in ["preferWebsockets", "omitMaxOutputTokens", "compactionModel"] {
        for source in ["models", "modelOverrides"] {
            let mut provider = custom("openai-completions");
            let value = if field == "compactionModel" { json!("custom/other") } else { json!(false) };
            if source == "models" {
                provider["models"][0][field] = value;
            } else {
                provider["modelOverrides"] = json!({"group/exact-model":{}});
                provider["modelOverrides"]["group/exact-model"][field] = value;
            }
            let cfg = config(provider);
            let error = resolve_daily_selection(Some(&cfg), &selected(), &no_env)
                .err()
                .expect("wire option is still unsupported");
            assert_eq!(error.field, format!("model/{field}"));
            assert_eq!(error.message, "is not supported by this daily wire adapter");
        }
    }
}

#[test]
fn read_decoder_publication_group() {
    read_decoder_authored_override_and_raw_daily_paths_group();
    read_decoder_materialized_donor_restore_and_role_selection_group();
    read_decoder_keeps_unimplemented_wire_options_rejected_group();
}
