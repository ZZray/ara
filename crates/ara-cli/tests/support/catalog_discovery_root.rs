//! Root discovery replay, with injected response semantics and retained signals.
use crate::{FixtureTransport, array, decode, failure, field, insert_record, pairs, s, specs_value, success, text};
use ara_cli::{
    catalog_discovery::{google::*, openai::*, *},
    model_collapse::VariantSpec,
};
use ara_rpc::{WireString, WireValue as W};
use async_trait::async_trait;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

fn optional(value: &VariantSpec, key: &str) -> Option<WireString> {
    value.get(key).and_then(W::as_string).cloned()
}
fn number(value: &VariantSpec, key: &str) -> Option<f64> {
    value.get(key).and_then(W::as_number)
}
fn model_result(value: DiscoveryResult) -> W {
    match value {
        Err(error) => failure(error),
        Ok(None) => success(&VariantSpec::from_wire(W::Null)),
        Ok(Some(models)) => success(&specs_value(&models)),
    }
}
struct Environment {
    values: HashMap<String, WireString>,
}
impl GoogleHeaderEnvironment for Environment {
    fn get(&self, name: &str) -> Option<WireString> {
        self.values.get(name).cloned()
    }
    fn platform(&self) -> WireString {
        ProcessGoogleHeaderEnvironment.platform()
    }
    fn arch(&self) -> WireString {
        ProcessGoogleHeaderEnvironment.arch()
    }
}
struct Transport {
    inner: FixtureTransport,
    signals: Mutex<Vec<DiscoverySignal>>,
}
#[async_trait]
impl DiscoveryTransport for Transport {
    fn context_id(&self) -> u64 {
        173
    }
    async fn fetch(&self, request: DiscoveryRequest) -> Result<DiscoveryReply, DiscoveryError> {
        if let Some(signal) = &request.signal {
            self.signals.lock().unwrap().push(signal.clone());
        }
        self.inner.fetch(request).await
    }
}
pub async fn replay(test: &W) -> W {
    let options = test.get("optionsEncoded").map(decode).unwrap_or_else(|| {
        VariantSpec::from_wire(test.get("options").cloned().unwrap_or_else(|| W::Object(Vec::new())))
    });
    let transport = Arc::new(Transport { inner: FixtureTransport::new(test), signals: Mutex::new(Vec::new()) });
    let mut context = CatalogContext::new(transport.clone()).unwrap();
    let values = test
        .get("env")
        .and_then(W::entries)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(key, value)| Some((key.to_utf8().ok()?, value.as_string()?.clone())))
        .collect();
    context.runtime = Arc::new(DiscoveryRuntime {
        collapse: Mutex::new(ara_cli::model_collapse::CollapseRuntime::new().unwrap()),
        google_headers: Arc::new(GoogleHeaderRuntime::new(Arc::new(Environment { values }))),
        diagnostics: Mutex::new(Vec::new()),
    });
    let signal = test.get("signal").map(|configuration| {
        let signal = DiscoverySignal::default();
        if matches!(configuration.get("aborted"), Some(W::Bool(true))) {
            signal.abort(DiscoveryError::named("AbortError", "This operation was aborted."));
        }
        signal
    });
    let operation = test.get("op").map(text).unwrap_or_default();
    let family = text(field(test, "family"));
    if operation == "constants" && family != "google-headers" {
        let value = if family == "openai" {
            W::object(vec![("defaultTimeout", W::Number(DEFAULT_OPENAI_COMPATIBLE_DISCOVERY_TIMEOUT_MS))])
        } else {
            W::object(vec![
                ("primaryEndpoint", s(ANTIGRAVITY_PRIMARY_ENDPOINT)),
                ("sandboxEndpoint", s(ANTIGRAVITY_SANDBOX_ENDPOINT)),
            ])
        };
        return transport.inner.finish(success(&VariantSpec::from_wire(value)));
    }
    let result = match family.as_str() {
        "openai" => {
            let mut config = OpenAiCompatibleOptions::new(
                optional(&options, "api").unwrap_or_else(|| "openai-completions".into()),
                optional(&options, "provider").unwrap_or_else(|| "fixture".into()),
                optional(&options, "baseUrl").unwrap_or_else(|| "".into()),
            );
            config.api_key = optional(&options, "apiKey");
            config.headers = pairs(options.get("headers"));
            config.timeout_ms = number(&options, "timeoutMs");
            config.signal = signal.clone();
            if test.get("mapper").map(text).as_deref() == Some("decorate") {
                config.map_model = Some(Arc::new(|entry, defaults, context| {
                    if matches!(entry.get("throw"), Some(W::Bool(true))) {
                        return Err(DiscoveryError::new("mapper failed"));
                    }
                    if matches!(entry.get("skip"), Some(W::Bool(true))) {
                        return Ok(None);
                    }
                    let mut model = (*defaults).clone();
                    if let Some(id) = entry.get("mappedId") {
                        model.set("id", id.clone());
                    }
                    if matches!(entry.get("ctx"), Some(W::Bool(true))) {
                        model.set(
                            "extraContext",
                            W::object(vec![
                                ("api", s(context.api.clone())),
                                ("provider", s(context.provider.clone())),
                                ("baseUrl", s(context.base_url.clone())),
                            ]),
                        );
                    }
                    Ok(Some(Arc::new(model)))
                }));
            }
            match test.get("filter").map(text).as_deref() {
                Some("exclude") => {
                    config.filter_model =
                        Some(Arc::new(|entry, _| Ok(!matches!(entry.get("keep"), Some(W::Bool(false))))))
                }
                Some("throw") => config.filter_model = Some(Arc::new(|_, _| Err(DiscoveryError::new("filter failed")))),
                _ => {}
            }
            model_result(fetch_openai_compatible_models(&context, &config).await)
        }
        "gemini" => {
            let mut config = GeminiDiscoveryOptions::new(optional(&options, "apiKey").unwrap_or_else(|| "".into()));
            config.base_url = optional(&options, "baseUrl");
            config.page_size = number(&options, "pageSize");
            config.max_pages = number(&options, "maxPages");
            config.signal = signal.clone();
            model_result(fetch_gemini_models(&context, &config).await)
        }
        "antigravity" => {
            let mut config = AntigravityDiscoveryOptions::new(optional(&options, "token").unwrap_or_else(|| "".into()));
            config.endpoint = optional(&options, "endpoint");
            config.project = optional(&options, "project");
            config.user_agent = optional(&options, "userAgent");
            config.signal = signal.clone();
            model_result(fetch_antigravity_discovery_models(&context, &config).await)
        }
        "gemini-cli" => {
            let mut config = GeminiCliQuotaOptions::new(optional(&options, "token").unwrap_or_else(|| "".into()));
            config.endpoint = optional(&options, "endpoint");
            config.project_id = optional(&options, "projectId");
            config.signal = signal.clone();
            model_result(fetch_gemini_cli_quota_models(&context, &config).await)
        }
        "google-headers" => {
            let args = test.get("args").map(array).unwrap_or_default();
            let headers = &context.runtime.google_headers;
            if operation == "getAntigravityModelWireProfile"
                && get_antigravity_model_wire_profile(args[0].as_string().unwrap()).is_none()
            {
                return transport
                    .inner
                    .finish(success(&VariantSpec { value: W::Null, undefined_paths: vec![Vec::new()] }));
            }
            let result = match operation.as_str() {
                "constants" => {
                    let ids = [
                        "gemini-3.5-flash-extra-low",
                        "gemini-3.5-flash-low",
                        "gemini-3-flash-agent",
                        "gemini-3.1-pro-low",
                        "gemini-pro-agent",
                        "claude-sonnet-4-6",
                        "claude-opus-4-6-thinking",
                    ];
                    let profiles = ids
                        .into_iter()
                        .map(|id| {
                            let profile = get_antigravity_model_wire_profile(&id.into()).unwrap();
                            let mut entries = Vec::new();
                            if let Some(model_enum) = profile.model_enum {
                                entries.push(("modelEnum".into(), s(model_enum)));
                            }
                            entries.push(("maxOutputTokens".into(), W::Number(f64::from(profile.max_output_tokens))));
                            (id.into(), W::Object(entries))
                        })
                        .collect();
                    W::object(vec![
                        ("defaultVersion", s(DEFAULT_ANTIGRAVITY_VERSION)),
                        ("profiles", W::Object(profiles)),
                    ])
                }
                "ensure-sequence" => {
                    let mut snapshots = Vec::new();
                    for step in array(field(test, "steps")) {
                        match text(step).as_str() {
                            "ensure" => headers.ensure_antigravity_version(&context, signal.clone()).await,
                            "parallel" => {
                                tokio::join!(
                                    headers.ensure_antigravity_version(&context, signal.clone()),
                                    headers.ensure_antigravity_version(&context, signal.clone())
                                );
                            }
                            "abort" => signal
                                .as_ref()
                                .unwrap()
                                .abort(DiscoveryError::named("AbortError", "This operation was aborted.")),
                            "snapshot" => snapshots.push(W::object(vec![
                                ("version", s(headers.get_antigravity_version())),
                                ("userAgent", s(headers.get_antigravity_user_agent())),
                                (
                                    "retainedSignals",
                                    W::Array(
                                        transport
                                            .signals
                                            .lock()
                                            .unwrap()
                                            .iter()
                                            .map(|signal| W::Bool(signal.is_aborted()))
                                            .collect(),
                                    ),
                                ),
                            ])),
                            step => panic!("unknown header step {step}"),
                        }
                    }
                    W::Array(snapshots)
                }
                "parseAntigravityManifestVersion" => {
                    parse_antigravity_manifest_version(args[0].as_string().unwrap()).map(W::String).unwrap_or(W::Null)
                }
                "getGeminiCliUserAgent" => s(headers.get_gemini_cli_user_agent(args.first().and_then(W::as_string))),
                "getGeminiCliHeaders" => W::Object(
                    headers
                        .get_gemini_cli_headers(args.first().and_then(W::as_string))
                        .into_iter()
                        .map(|(key, value)| (key, s(value)))
                        .collect(),
                ),
                "getAntigravityVersion" => s(headers.get_antigravity_version()),
                "getAntigravityUserAgent" => s(headers.get_antigravity_user_agent()),
                "getAntigravityModelWireProfile" => get_antigravity_model_wire_profile(args[0].as_string().unwrap())
                    .map(|profile| {
                        let mut entries = Vec::new();
                        if let Some(model_enum) = profile.model_enum {
                            entries.push(("modelEnum".into(), s(model_enum)));
                        }
                        entries.push(("maxOutputTokens".into(), W::Number(f64::from(profile.max_output_tokens))));
                        W::Object(entries)
                    })
                    .unwrap_or(W::Null),
                "ensureAntigravityVersion" => {
                    headers.ensure_antigravity_version(&context, signal.clone()).await;
                    s(headers.get_antigravity_version())
                }
                _ => panic!("unhandled root operation {operation}"),
            };
            success(&VariantSpec::from_wire(result))
        }
        _ => panic!("unhandled root family {family}"),
    };
    if let Some(delay) = test.get("afterDelayMs").and_then(W::as_number) {
        tokio::time::sleep(std::time::Duration::from_millis(delay as u64)).await;
    }
    let result = if matches!(test.get("observeSignals"), Some(W::Bool(true))) {
        let mut value = VariantSpec::from_wire(W::Object(Vec::new()));
        insert_record(&mut value, "models", &decode(field(&result, "value")));
        value.set(
            "retainedSignals",
            W::Array(transport.signals.lock().unwrap().iter().map(|signal| W::Bool(signal.is_aborted())).collect()),
        );
        success(&value)
    } else {
        result
    };
    transport.inner.finish(result)
}
