//! Module scenarios for explicit daily CLI configuration. Socket/login/session
//! scenarios belong to the CLI/auth modules; these use the real file loader and
//! the actual native request encoders, without process environment mutation.

use ara_ai::providers::{openai_completions as chat, openai_responses as responses};
use ara_ai::{Context, DeveloperMessage, Message, UserContent, UserMessage};
use ara_cli::daily_model_config::{
    DailyApi, DailyAuthSource, DailyOverrides, load_daily_config, resolve_daily_promotion_selection,
    resolve_daily_selection,
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
        let body = chat::build_params(&selection.model, &context(), options);
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
    assert_eq!(current.metadata.as_object().unwrap().len(), 6);
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
        let missing_target_env = |name: &str| if name == "ARA_TEST_API_KEY" { None } else { env(name) };
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
        Message::Developer(DeveloperMessage { content: UserContent::Text("Be concise".into()), timestamp: 0 }),
    );
    let body = chat::build_params(&route.model, &input, options);
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
fn unsupported_selected_semantics_and_missing_selection_fail_explicitly() {
    let cli = selected();
    for (key, value) in [
        ("preferWebsockets", json!(true)),
        ("thinking", json!({"mode":"effort","efforts":["high"]})),
        ("omitMaxOutputTokens", json!(true)),
    ] {
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
