//! Module scenarios for explicit daily CLI configuration. Socket/login/session
//! scenarios belong to the CLI/auth modules; these use the real file loader and
//! the actual native request encoders, without process environment mutation.

use ara_ai::providers::{openai_completions as chat, openai_responses as responses};
use ara_ai::{Context, DeveloperMessage, Message, UserContent, UserMessage};
use ara_cli::daily_model_config::{
    DailyApi, DailyAuthSource, DailyOverrides, load_daily_config, resolve_daily_selection,
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
fn chat_selection_precedence_reaches_encoder_and_keeps_auth_private() {
    let mut value = custom("openai-completions");
    value["compat"] =
        json!({"supportsDeveloperRole":true,"supportsUsageInStreaming":false,"maxTokensField":"max_completion_tokens"});
    value["headers"] = json!({"X-Route":"provider","Authorization":"Bearer fixture"});
    value["models"][0]["baseUrl"] = json!("https://model.example/v1");
    value["models"][0]["headers"] = json!({"x-route":"model"});
    value["models"][0]["compat"] = json!({"supportsUsageInStreaming":true,"streamIdleTimeoutMs":1250});
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
    assert_eq!(route.model.id, "group/exact-model");
    assert_eq!(route.model.base_url, "https://cli.example/v1");
    assert_eq!(route.generation.max_tokens, Some(64));
    let ProtocolOptions::Completions(options) = &route.protocol else { panic!("Chat protocol") };
    assert!(options.api_key.is_none());
    assert_eq!(options.extra_headers, vec![("x-route".into(), "cli".into())]);
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
    let DailyAuthSource::Fixed(lease) = route.auth_source else { panic!("fixed lease") };
    assert_eq!(lease.identity(), &CredentialIdentity::Environment { variable: "EXPLICIT_KEY".into() });
    assert!(PreparedRoute::new(route.model, route.protocol, Arc::new(FixedRequestAuth::new(lease)), 0).is_ok());
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
    provider["apiKey"] = json!("!echo private-command");
    let cfg = config(provider);
    let failure = resolve_daily_selection(Some(&cfg), &cli, &no_env).err().unwrap().to_string();
    assert!(failure.contains("apiKey"));
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
