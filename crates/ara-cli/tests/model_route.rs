//! Real adapter requests for complete host route/credential binding. These are
//! controlled upstream tests, not full registry/OAuth/fallback acceptance.

use ara_ai::{AssistantMessage, CallOptions, Context, Message, Model, StopReason, UserMessage};
use ara_cli::model_route::*;
use ara_testkit::{FakeUpstream, Script};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

// Fixed provider-response.test.ts inputs, plus real Host and no-replay boundaries.
#[tokio::test]
async fn provider_response_notifications_follow_final_success_and_await_callback() {
    // Headers and the first semantic body item have separate native budgets.
    for api in ["openai-completions", "openai-responses"] {
        let mut scripted =
            if api == "openai-completions" { chat("fresh body budget") } else { responses("fresh body budget") };
        scripted["delay_ms"] = json!(250);
        let events = scripted["events"].as_array_mut().unwrap();
        events.insert(0, json!({"sleep_ms":250}));
        events.insert(0, json!({"raw":": keepalive\n\n"}));
        let server = server(vec![scripted]).await;
        let model = model(api, "openai", server.base_url());
        let mut protocol = protocol(api);
        match &mut protocol {
            ProtocolOptions::Completions(options) => options.first_event_timeout = Some(Duration::from_millis(400)),
            ProtocolOptions::Responses(options) => options.first_event_timeout = Some(Duration::from_millis(400)),
            _ => unreachable!(),
        }
        let provider = PreparedRoute::new(
            model.clone(),
            protocol,
            Arc::new(FixedRequestAuth::new(RequestAuthLease::new(
                CredentialIdentity::Runtime,
                Some("fixture-key".into()),
            ))),
            0,
        )
        .unwrap()
        .bind(reqwest::Client::new(), None);
        assert_eq!(
            collect(provider.stream(&model, &context(), CallOptions::default())).await.stop_reason,
            StopReason::Stop
        );
        assert_eq!(server.served(), 1);
    }

    for api in ["openai-completions", "openai-responses"] {
        let mut success = if api == "openai-completions" { chat("observed") } else { responses("observed") };
        success["status"] = json!(202);
        success["headers"] = json!({"X-Request-ID":"req_stream_simple","X-RateLimit-Remaining":"42",
            "Authorization":"response-private-marker"});
        let server = server(vec![json!({"status":503,"body":"temporarily unavailable"}), success]).await;
        let model = model(api, "openai", server.base_url());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let entered = Arc::new(tokio::sync::Semaphore::new(0));
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let callback = ara_ai::ProviderResponseCallback::new({
            let seen = seen.clone();
            let entered = entered.clone();
            let release = release.clone();
            move |response, observed_model| {
                let seen = seen.clone();
                let entered = entered.clone();
                let release = release.clone();
                Box::pin(async move {
                    seen.lock().unwrap().push((response, observed_model));
                    entered.add_permits(1);
                    release.acquire().await.unwrap().forget();
                    Ok(())
                })
            }
        });
        let provider = PreparedRoute::new(
            model.clone(),
            protocol(api),
            Arc::new(FixedRequestAuth::new(RequestAuthLease::new(
                CredentialIdentity::Runtime,
                Some("fixture-key".into()),
            ))),
            0,
        )
        .unwrap()
        .bind(reqwest::Client::new(), None);
        let mut stream =
            provider.stream(&model, &context(), CallOptions { on_response: Some(callback), ..Default::default() });
        tokio::time::timeout(Duration::from_secs(3), entered.acquire()).await.unwrap().unwrap().forget();
        assert!(matches!(stream.try_recv(), Err(tokio::sync::mpsc::error::TryRecvError::Empty)));
        assert_eq!(server.served(), 2, "unsuccessful attempt is not a response notification");
        {
            let observed = seen.lock().unwrap();
            assert_eq!(observed.len(), 1);
            assert_eq!(observed[0].0.status, 202);
            assert_eq!(observed[0].0.request_id, Some(Some("req_stream_simple".into())));
            assert_eq!(observed[0].0.headers["x-ratelimit-remaining"], "42");
            assert_eq!(observed[0].1.as_ref(), Some(&model));
        }
        release.add_permits(1);
        let terminal = collect(stream).await;
        assert_eq!(terminal.stop_reason, StopReason::Stop);
        assert!(!serde_json::to_string(&terminal).unwrap().contains("response-private-marker"));
    }

    for api in ["openai-completions", "openai-responses"] {
        let scripted = if api == "openai-completions" { chat("after callback") } else { responses("after callback") };
        let server = server(vec![scripted]).await;
        let model = model(api, "openai", server.base_url());
        let settled = Arc::new(AtomicUsize::new(0));
        let callback = ara_ai::ProviderResponseCallback::new({
            let settled = settled.clone();
            move |_, _| {
                let settled = settled.clone();
                Box::pin(async move {
                    tokio::time::sleep(Duration::from_millis(350)).await;
                    settled.fetch_add(1, Ordering::AcqRel);
                    Ok(())
                })
            }
        });
        let mut protocol = protocol(api);
        match &mut protocol {
            ProtocolOptions::Completions(options) => options.first_event_timeout = Some(Duration::from_millis(200)),
            ProtocolOptions::Responses(options) => options.first_event_timeout = Some(Duration::from_millis(200)),
            _ => unreachable!(),
        }
        let provider = PreparedRoute::new(
            model.clone(),
            protocol,
            Arc::new(FixedRequestAuth::new(RequestAuthLease::new(
                CredentialIdentity::Runtime,
                Some("fixture-key".into()),
            ))),
            0,
        )
        .unwrap()
        .bind(reqwest::Client::new(), None);
        let output = collect(provider.stream(
            &model,
            &context(),
            CallOptions { on_response: Some(callback), ..Default::default() },
        ))
        .await;
        assert_eq!(settled.load(Ordering::Acquire), 1, "timeout does not discard callback work");
        assert_eq!(server.served(), 1, "a callback deadline cannot replay the successful POST");
        if api == "openai-completions" {
            assert_eq!(output.stop_reason, StopReason::Error);
            let evidence = output.failure_evidence.unwrap();
            assert!(evidence.replay_blocked && evidence.same_route_blocked);
        } else {
            assert_eq!(output.stop_reason, StopReason::Stop);
        }
    }
}

struct CallbackRetryAuth(AtomicUsize);
#[async_trait]
impl RequestAuthResolver for CallbackRetryAuth {
    async fn resolve(&self, _: &Model, _: &CancellationToken) -> Result<RequestAuthLease, AuthResolveError> {
        Ok(RequestAuthLease::new(CredentialIdentity::Stored { id: 1, revision: 1 }, Some("fixture-key".into())))
    }
    fn supports_auth_refresh(&self) -> bool {
        true
    }
    async fn refresh(&self, _: &Model, _: &CancellationToken) -> Result<Option<RequestAuthLease>, AuthResolveError> {
        self.0.fetch_add(1, Ordering::AcqRel);
        Ok(Some(RequestAuthLease::new(CredentialIdentity::Stored { id: 1, revision: 2 }, Some("changed-key".into()))))
    }
}

#[tokio::test]
async fn provider_response_callback_failures_cannot_authorize_auth_or_model_replay() {
    for api in ["openai-completions", "openai-responses"] {
        for status in [401, 429, 503] {
            let success = if api == "openai-completions" { chat("unused") } else { responses("unused") };
            let server = server(vec![success.clone(), success]).await;
            let model = model(api, "openai", server.base_url());
            let auth = Arc::new(CallbackRetryAuth(AtomicUsize::new(0)));
            let calls = Arc::new(AtomicUsize::new(0));
            let callback = ara_ai::ProviderResponseCallback::new({
                let calls = calls.clone();
                move |_, _| {
                    calls.fetch_add(1, Ordering::AcqRel);
                    Box::pin(async move {
                        Err(ara_ai::ProviderError::Http {
                            status,
                            detail: "invalidated OAuth token; usage limit; retry your request".into(),
                        })
                    })
                }
            });
            let provider = PreparedRoute::new(model.clone(), protocol(api), auth.clone(), 0)
                .unwrap()
                .bind(reqwest::Client::new(), None);
            let terminal = collect(provider.stream(
                &model,
                &context(),
                CallOptions { on_response: Some(callback), ..Default::default() },
            ))
            .await;
            assert_eq!(terminal.stop_reason, StopReason::Error);
            assert_eq!(server.served(), 1);
            assert_eq!(calls.load(Ordering::Acquire), 1);
            assert_eq!(auth.0.load(Ordering::Acquire), 0);
            let evidence = terminal.failure_evidence.unwrap();
            assert_eq!(evidence.kind, ara_ai::retry_classification::ProviderErrorKind::Config);
            assert!(evidence.replay_blocked && evidence.same_route_blocked);
        }
    }
}

struct HeaderFixtureUsage;
#[async_trait]
impl ara_cli::auth_storage::UsageProvider for HeaderFixtureUsage {
    async fn fetch_usage(
        &self,
        _: ara_cli::auth_storage::UsageRequest,
        _: &CancellationToken,
    ) -> Result<Option<ara_cli::auth_storage_policy::UsageReport>, ara_cli::auth_storage::UsageFetchError> {
        Ok(ara_cli::codex_usage::parse_codex_rate_limit_headers(&quota_headers(1), ara_ai::now_ms() as f64))
    }
    fn rate_limit_header_parser(&self) -> Option<ara_cli::auth_storage::RateLimitHeaderParser> {
        Some(ara_cli::codex_usage::parse_codex_rate_limit_headers)
    }
}
fn quota_headers(used: u32) -> std::collections::BTreeMap<String, String> {
    [
        ("x-codex-primary-used-percent".into(), used.to_string()),
        ("x-codex-primary-window-minutes".into(), "300".into()),
        ("x-codex-primary-reset-at".into(), ((ara_ai::now_ms() / 1000) + 3600).to_string()),
        ("x-codex-secondary-used-percent".into(), used.to_string()),
        ("x-codex-secondary-window-minutes".into(), "10080".into()),
        ("x-codex-secondary-reset-at".into(), ((ara_ai::now_ms() / 1000) + 3600).to_string()),
    ]
    .into()
}

/// Existing custom ModelProvider port, equivalent to native AgentOptions.streamFn.
struct HeaderExtension;
impl ara_ai::ModelProvider for HeaderExtension {
    fn stream(&self, model: &Model, _: &Context, options: CallOptions) -> ara_ai::AssistantStream {
        let (sender, receiver) = tokio::sync::mpsc::channel(2);
        let model = model.clone();
        tokio::spawn(async move {
            let response = ara_ai::ProviderResponseMetadata {
                status: 200,
                headers: quota_headers(100),
                request_id: Some(None),
                metadata: None,
            };
            let result = options
                .on_response
                .expect("Host installs extension callback")
                .notify(response, Some(model.clone()))
                .await;
            let mut output = AssistantMessage::empty(&model.api, &model.provider, &model.id);
            let event = if let Err(error) = result {
                output.stop_reason = StopReason::Error;
                output.error_message = Some(error.to_string());
                ara_ai::AssistantMessageEvent::Error { reason: StopReason::Error, error: output }
            } else {
                output.stop_reason = StopReason::Stop;
                ara_ai::AssistantMessageEvent::Done { reason: StopReason::Stop, message: output }
            };
            let _ = sender.send(event).await;
        });
        receiver
    }
}

#[tokio::test]
async fn host_usage_callback_rotates_current_account_and_codex_http_remains_silent() {
    use ara_cli::{
        auth_storage::{AuthRequestContext, AuthStorage, AuthStorageOptions},
        credential_store::{AuthCredential, SqliteCredentialStore},
        openai_codex_auth::OpenAiCodexAuth,
        session_usage_headers::SessionUsageHeaders,
    };
    let dir = tempfile::tempdir().unwrap();
    let codex = Arc::new(OpenAiCodexAuth::open(dir.path().join("auth.db"), reqwest::Client::new()).await.unwrap());
    // An independent SQLite connection observes the real durable store without
    // exposing the account owner's private handle as a new public interface.
    let store = Arc::new(Mutex::new(SqliteCredentialStore::open(dir.path().join("auth.db")).unwrap()));
    let rows = store
        .lock()
        .unwrap()
        .replace_auth_credentials_for_provider(
            "openai-codex",
            &[
                AuthCredential::oauth(
                    json!({"access":"fixture-A","refresh":"refresh-A","expires":ara_ai::now_ms()+3600000,
            "accountId":"account-A","email":"a@fixture"})
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
                AuthCredential::oauth(
                    json!({"access":"fixture-B","refresh":"refresh-B","expires":ara_ai::now_ms()+3600000,
            "accountId":"account-B","email":"b@fixture"})
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
            ],
        )
        .unwrap();
    let storage =
        AuthStorage::for_codex(codex.clone(), AuthStorageOptions { jitter: Arc::new(|| 0.5), ..Default::default() })
            .unwrap();
    storage.register_usage_provider("openai-codex", Arc::new(HeaderFixtureUsage)).unwrap();
    let base = "https://chatgpt.com/backend-api";
    let context = AuthRequestContext {
        session_id: Some("logical-session".into()),
        base_url: Some(base.into()),
        ..Default::default()
    };
    let cancel = CancellationToken::new();
    storage.get_api_key("openai-codex", &context, &cancel).await.unwrap().unwrap();
    assert_eq!(
        storage.peek_oauth_access("openai-codex", Some("logical-session")).unwrap().unwrap().credential_id,
        rows[0].id
    );
    let owner = SessionUsageHeaders::new(storage.clone(), context.session_id.clone(), Some(base.into()));
    let extension = owner.bind(Arc::new(HeaderExtension));
    let fixture_model = model("openai-codex-responses", "openai-codex", base.into());
    assert_eq!(
        collect(extension.stream(&fixture_model, &self::context(), CallOptions::default())).await.stop_reason,
        StopReason::Stop
    );
    storage.get_api_key("openai-codex", &context, &cancel).await.unwrap().unwrap();
    assert_eq!(
        storage.peek_oauth_access("openai-codex", Some("logical-session")).unwrap().unwrap().credential_id,
        rows[1].id
    );

    // Native attribution follows the active Session row at callback time.
    storage.pin_session_oauth_account("openai-codex", "new-session", rows[1].id, None).unwrap();
    owner
        .for_session("new-session")
        .ingest_provider_usage_headers(
            &ara_ai::ProviderResponseMetadata {
                status: 429,
                headers: quota_headers(100),
                request_id: None,
                metadata: None,
            },
            Some(&fixture_model),
        )
        .unwrap();
    owner
        .ingest_provider_usage_headers(
            &ara_ai::ProviderResponseMetadata {
                status: 204,
                headers: quota_headers(100),
                request_id: Some(None),
                metadata: None,
            },
            None,
        )
        .unwrap();

    // Host interception consumes quota before the chosen configured/per-call
    // callback; an explicit per-call callback retains the adapter precedence.
    for (index, (api, per_call)) in [
        ("openai-completions", false),
        ("openai-completions", true),
        ("openai-responses", false),
        ("openai-responses", true),
    ]
    .into_iter()
    .enumerate()
    {
        let minutes = 17 + index;
        let mut headers = quota_headers(100);
        headers.insert("x-codex-primary-window-minutes".into(), minutes.to_string());
        let mut scripted =
            if api == "openai-completions" { chat("callback order") } else { responses("callback order") };
        scripted["headers"] = serde_json::to_value(headers).unwrap();
        let server = server(vec![scripted]).await;
        let model = model(api, "openai-codex", server.base_url());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let make_callback = |label: &'static str| {
            let store = store.clone();
            let seen = seen.clone();
            ara_ai::ProviderResponseCallback::new(move |_, _| {
                let store = store.clone();
                let seen = seen.clone();
                Box::pin(async move {
                    let cache = store.lock().unwrap().get_cache(
                        "usage_cache:report:openai-codex:https://chatgpt.com/backend-api:oauth|account:account-B|email:b@fixture",
                        true,
                    ).unwrap().unwrap();
                    let cache: Value = serde_json::from_str(&cache).unwrap();
                    assert_eq!(
                        cache["value"]["limits"][0]["window"]["durationMs"].as_f64(),
                        Some((minutes * 60_000) as f64)
                    );
                    seen.lock().unwrap().push(label);
                    Ok(())
                })
            })
        };
        let mut configured = protocol(api);
        match &mut configured {
            ProtocolOptions::Completions(options) => options.on_response = Some(make_callback("configured")),
            ProtocolOptions::Responses(options) => options.on_response = Some(make_callback("configured")),
            _ => unreachable!(),
        }
        let provider = PreparedRoute::new(
            model.clone(),
            configured,
            Arc::new(FixedRequestAuth::new(RequestAuthLease::new(
                CredentialIdentity::Runtime,
                Some("fixture-key".into()),
            ))),
            0,
        )
        .unwrap()
        .with_usage_headers(owner.for_session("new-session"))
        .bind(reqwest::Client::new(), None);
        let options = CallOptions { on_response: per_call.then(|| make_callback("per-call")), ..Default::default() };
        assert_eq!(collect(provider.stream(&model, &self::context(), options)).await.stop_reason, StopReason::Stop);
        assert_eq!(*seen.lock().unwrap(), vec![if per_call { "per-call" } else { "configured" }]);
    }

    let mut scripted = responses("ordinary Codex");
    scripted["headers"] = serde_json::to_value(quota_headers(100)).unwrap();
    let server = server(vec![scripted]).await;
    let model = model("openai-codex-responses", "openai-codex", server.base_url());
    let calls = Arc::new(AtomicUsize::new(0));
    let callback = ara_ai::ProviderResponseCallback::new({
        let calls = calls.clone();
        move |_, _| {
            calls.fetch_add(1, Ordering::AcqRel);
            Box::pin(async { Ok(()) })
        }
    });
    let route = PreparedRoute::new(
        model.clone(),
        ProtocolOptions::CodexResponses(Default::default()),
        Arc::new(FixedRequestAuth::new(
            RequestAuthLease::new(CredentialIdentity::Runtime, Some("fixture-key".into()))
                .with_headers(vec![("chatgpt-account-id".into(), "fixture-account".into())]),
        )),
        0,
    )
    .unwrap()
    .with_usage_headers(owner);
    let snapshot = store
        .lock()
        .unwrap()
        .get_cache(
            "usage_cache:report:openai-codex:https://chatgpt.com/backend-api:oauth|account:account-B|email:b@fixture",
            true,
        )
        .unwrap();
    let terminal = collect(route.bind_codex_session(reqwest::Client::new(), "new-session".into()).stream(
        &model,
        &self::context(),
        CallOptions { on_response: Some(callback), ..Default::default() },
    ))
    .await;
    assert_eq!(terminal.stop_reason, StopReason::Stop, "{terminal:?}");
    assert_eq!(calls.load(Ordering::Acquire), 0);
    assert_eq!(server.served(), 1);
    assert_eq!(snapshot, store.lock().unwrap().get_cache(
        "usage_cache:report:openai-codex:https://chatgpt.com/backend-api:oauth|account:account-B|email:b@fixture", true).unwrap());
}

fn model(api: &str, provider: &str, base_url: String) -> Model {
    Model {
        id: format!("{provider}-model"),
        api: api.into(),
        provider: provider.into(),
        base_url,
        reasoning: false,
        max_tokens: None,
        context_window: None,
        tokenizer: None,
    }
}

fn context() -> Context {
    Context {
        system_prompt: vec!["fixture instructions".into()],
        messages: vec![Message::User(UserMessage::text("hello"))],
        tools: None,
    }
}

fn chat(text: &str) -> Value {
    json!({"events":[
        {"data":{"choices":[{"delta":{"content":text}}]}},
        {"data":{"choices":[{"delta":{},"finish_reason":"stop"}]}},
        {"done":true}
    ]})
}

fn responses(text: &str) -> Value {
    json!({"events":[
        {"data":{"type":"response.output_item.done","output_index":0,"item":{
            "type":"message","id":"msg_fixture","content":[{"type":"output_text","text":text}]}}},
        {"data":{"type":"response.completed","response":{"id":"resp_fixture","status":"completed"}}}
    ]})
}

fn anthropic(text: &str) -> Value {
    let frames = [
        json!({"type":"message_start","message":{"id":"msg_fixture"}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":text}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}}),
        json!({"type":"message_stop"}),
    ]
    .into_iter()
    .map(|value| json!({"raw":format!("event: {}\ndata: {value}\n\n", value["type"].as_str().unwrap())}))
    .collect::<Vec<_>>();
    json!({"events":frames})
}

async fn server(responses: Vec<Value>) -> FakeUpstream {
    let script: Script = serde_json::from_value(json!({"responses":responses})).unwrap();
    FakeUpstream::start(script, None).await.unwrap()
}

fn protocol(api: &str) -> ProtocolOptions {
    let retry = ara_ai::providers::openai_completions::RetryPolicy {
        max_attempts: 2,
        base_delay: Duration::ZERO,
        max_delay: Duration::ZERO,
    };
    match api {
        "openai-completions" => ProtocolOptions::Completions(ara_ai::providers::openai_completions::StreamOptions {
            retry,
            ..Default::default()
        }),
        "openai-responses" => ProtocolOptions::Responses(ara_ai::providers::openai_responses::StreamOptions {
            retry,
            ..Default::default()
        }),
        "anthropic-messages" => {
            ProtocolOptions::Anthropic(ara_ai::providers::anthropic::StreamOptions { retry, ..Default::default() })
        }
        _ => unreachable!(),
    }
}

async fn collect(mut stream: ara_ai::AssistantStream) -> AssistantMessage {
    tokio::time::timeout(Duration::from_secs(5), async move {
        let mut terminals = Vec::new();
        while let Some(event) = stream.recv().await {
            if event.is_terminal() {
                terminals.push(event.partial().clone());
            }
        }
        assert_eq!(terminals.len(), 1);
        terminals.pop().unwrap()
    })
    .await
    .expect("bounded route stream")
}

#[tokio::test]
async fn codex_side_calls_preserve_cache_prefix_and_isolate_transport_state() {
    for cache_override in [None, Some("explicit-cache-key")] {
        let server =
            server(vec![responses("main"), responses("side one"), responses("side two"), responses("continued")]).await;
        let model = model("openai-codex-responses", "openai-codex", server.base_url());
        let auth = Arc::new(FixedRequestAuth::new(
            RequestAuthLease::new(CredentialIdentity::Runtime, Some("synthetic fixture token".into()))
                .with_headers(vec![("chatgpt-account-id".into(), "fixture-account".into())]),
        ));
        let mut options = ara_ai::providers::openai_codex_responses::StreamOptions {
            session_id: Some("main-session".into()),
            ..Default::default()
        };
        options.request.prompt_cache_key = cache_override.map(str::to_owned);
        let route = PreparedRoute::new(model.clone(), ProtocolOptions::CodexResponses(options), auth, 9).unwrap();
        let client = reqwest::Client::new();
        let main = route.bind(client.clone(), None);
        let side_one = route.bind_side_request(client.clone(), "main-session");
        let side_two = route.bind_side_request(client, "main-session");
        for (provider, expected) in
            [(&main, "main"), (&side_one, "side one"), (&side_two, "side two"), (&main, "continued")]
        {
            assert_eq!(collect(provider.stream(&model, &context(), CallOptions::default())).await.text(), expected);
        }
        let requests = server.requests.lock().await;
        assert_eq!(requests.len(), 4);
        for request in requests.iter() {
            assert_eq!(request["body"]["prompt_cache_key"], cache_override.unwrap_or("main-session"));
            assert_eq!(request["body"]["input"], requests[0]["body"]["input"]);
            assert_eq!(request["headers"]["chatgpt-account-id"], "fixture-account");
            assert_eq!(request["headers"]["session_id"], request["headers"]["conversation_id"]);
            assert_eq!(request["headers"]["session_id"], request["headers"]["x-client-request-id"]);
        }
        assert_eq!(requests[0]["headers"]["session_id"], "main-session");
        assert_eq!(requests[3]["headers"]["session_id"], "main-session");
        assert!(requests[1]["headers"]["session_id"].as_str().unwrap().starts_with("main-session:side:"));
        assert_ne!(requests[1]["headers"]["session_id"], requests[2]["headers"]["session_id"]);
    }
}

struct ChangingAuth(AtomicUsize);

#[async_trait]
impl RequestAuthResolver for ChangingAuth {
    async fn resolve(&self, _: &Model, _: &CancellationToken) -> Result<RequestAuthLease, AuthResolveError> {
        let n = self.0.fetch_add(1, Ordering::SeqCst);
        Ok(RequestAuthLease::new(
            CredentialIdentity::Stored { id: n as i64 + 10, revision: n as i64 },
            Some("k".repeat(11 + n)),
        ))
    }
}

#[derive(Default)]
struct Observer {
    started: Mutex<Vec<RequestIdentity>>,
    settled: Mutex<Vec<(RequestIdentity, StopReason)>>,
    interrupted: AtomicUsize,
    fail_start: bool,
    fail_settle: bool,
}

impl RequestReceiptObserver for Observer {
    fn started(&self, request: &RequestIdentity) -> Result<(), ReceiptError> {
        self.started.lock().unwrap().push(request.clone());
        if self.fail_start { Err(ReceiptError) } else { Ok(()) }
    }
    fn settled(&self, request: &RequestIdentity, message: &AssistantMessage) -> Result<(), ReceiptError> {
        self.settled.lock().unwrap().push((request.clone(), message.stop_reason));
        if self.fail_settle { Err(ReceiptError) } else { Ok(()) }
    }
    fn interrupted(&self, _: &RequestIdentity) {
        self.interrupted.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn every_call_resolves_once_and_both_wire_retry_layers_keep_its_identity() {
    // First logical call: two HTTP attempts. Second: exhausted HTTP budget is
    // replayed by the provider's outer replay-safe retry, with the same lease.
    let server = server(vec![
        json!({"status":500,"body":"temporary"}),
        chat("first"),
        json!({"status":500,"body":"temporary"}),
        json!({"status":500,"body":"temporary"}),
        chat("second"),
    ])
    .await;
    let model = model("openai-completions", "fixture", server.base_url());
    let auth = Arc::new(ChangingAuth(AtomicUsize::new(0)));
    let observer = Arc::new(Observer::default());
    let route = PreparedRoute::new(model.clone(), protocol(&model.api), auth.clone(), 7).unwrap();
    let provider = route.bind(reqwest::Client::new(), Some(observer.clone()));
    assert_eq!(auth.0.load(Ordering::SeqCst), 0, "preparation does not resolve auth");
    assert_eq!(collect(provider.stream(&model, &context(), CallOptions::default())).await.text(), "first");
    assert_eq!(collect(provider.stream(&model, &context(), CallOptions::default())).await.text(), "second");
    assert_eq!(auth.0.load(Ordering::SeqCst), 2);
    let requests = server.requests.lock().await;
    assert_eq!(requests.len(), 5);
    assert_eq!(requests[0]["headers"]["authorization"], "<redacted 18 chars>");
    assert_eq!(requests[1]["headers"]["authorization"], requests[0]["headers"]["authorization"]);
    assert_eq!(requests[2]["headers"]["authorization"], "<redacted 19 chars>");
    assert_eq!(requests[3]["headers"]["authorization"], requests[2]["headers"]["authorization"]);
    assert_eq!(requests[4]["headers"]["authorization"], requests[2]["headers"]["authorization"]);
    let started = observer.started.lock().unwrap();
    let settled = observer.settled.lock().unwrap();
    assert_eq!(started.len(), 2);
    assert_eq!(settled.len(), 2);
    for (index, request) in started.iter().enumerate() {
        assert_eq!(request.route_generation, 7);
        assert_eq!(request.credential, CredentialIdentity::Stored { id: index as i64 + 10, revision: index as i64 });
        assert_eq!(settled[index].0, *request);
    }
    assert_ne!(started[0].call_id, started[1].call_id);
}

#[tokio::test]
async fn complete_targets_use_their_own_api_endpoint_authentication_and_options() {
    for (api, provider_name, reply, endpoint, header, expected_length) in [
        ("openai-completions", "chat-fixture", chat("chat"), "/v1/chat/completions", "authorization", 20),
        ("openai-responses", "responses-fixture", responses("responses"), "/v1/responses", "authorization", 20),
        ("anthropic-messages", "anthropic-proxy", anthropic("anthropic"), "/v1/messages", "authorization", 20),
        ("anthropic-messages", "opencode-go", anthropic("anthropic"), "/v1/messages", "x-api-key", 13),
    ] {
        let server = server(vec![reply]).await;
        let model = model(api, provider_name, server.base_url());
        let auth = Arc::new(FixedRequestAuth::new(RequestAuthLease::new(
            CredentialIdentity::Runtime,
            Some("fixture-value".into()),
        )));
        let route = PreparedRoute::new(model.clone(), protocol(api), auth, 4).unwrap();
        let result =
            collect(route.bind(reqwest::Client::new(), None).stream(&model, &context(), CallOptions::default())).await;
        assert_eq!(result.stop_reason, StopReason::Stop);
        assert_eq!(result.provider, provider_name);
        let requests = server.requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["request"], format!("POST {endpoint} HTTP/1.1"));
        assert_eq!(requests[0]["body"]["model"], model.id);
        assert_eq!(requests[0]["headers"][header], format!("<redacted {expected_length} chars>"));
        if header == "x-api-key" {
            assert!(requests[0]["headers"].get("authorization").is_none());
        } else {
            assert!(requests[0]["headers"].get("x-api-key").is_none());
        }
        if api == "anthropic-messages" {
            assert_eq!(requests[0]["body"]["system"][0]["text"], "fixture instructions");
        }
        if api == "openai-responses" {
            assert_eq!(requests[0]["body"]["store"], false);
        }
    }
}

#[tokio::test]
async fn a_late_request_terminal_is_attributed_to_its_acquired_credential() {
    let mut delayed = chat("late A");
    delayed["delay_ms"] = json!(100);
    let server = server(vec![delayed, chat("B")]).await;
    let model = model("openai-completions", "fixture", server.base_url());
    let auth = Arc::new(ChangingAuth(AtomicUsize::new(0)));
    let observer = Arc::new(Observer::default());
    let provider = PreparedRoute::new(model.clone(), protocol(&model.api), auth.clone(), 8)
        .unwrap()
        .bind(reqwest::Client::new(), Some(observer.clone()));
    let a = provider.stream(&model, &context(), CallOptions::default());
    tokio::time::timeout(Duration::from_secs(2), async {
        while server.served() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let b = provider.stream(&model, &context(), CallOptions::default());
    assert_eq!(collect(b).await.text(), "B");
    assert_eq!(collect(a).await.text(), "late A");
    let settled = observer.settled.lock().unwrap();
    assert_eq!(settled[0].0.credential, CredentialIdentity::Stored { id: 11, revision: 1 });
    assert_eq!(settled[1].0.credential, CredentialIdentity::Stored { id: 10, revision: 0 });
}

struct PendingAuth(Arc<tokio::sync::Notify>);
#[async_trait]
impl RequestAuthResolver for PendingAuth {
    async fn resolve(&self, _: &Model, _: &CancellationToken) -> Result<RequestAuthLease, AuthResolveError> {
        self.0.notify_one();
        std::future::pending().await
    }
}

#[tokio::test]
async fn cancellation_during_auth_has_one_aborted_terminal_and_no_request() {
    let server = server(vec![chat("unexpected")]).await;
    let model = model("openai-completions", "fixture", server.base_url());
    let notify = Arc::new(tokio::sync::Notify::new());
    let observer = Arc::new(Observer::default());
    let provider = PreparedRoute::new(model.clone(), protocol(&model.api), Arc::new(PendingAuth(notify.clone())), 0)
        .unwrap()
        .bind(reqwest::Client::new(), Some(observer.clone()));
    let cancel = CancellationToken::new();
    let stream = provider.stream(&model, &context(), CallOptions { cancel: cancel.clone(), ..Default::default() });
    tokio::time::timeout(Duration::from_secs(2), notify.notified()).await.unwrap();
    cancel.cancel();
    assert_eq!(collect(stream).await.stop_reason, StopReason::Aborted);
    assert_eq!(server.served(), 0);
    assert!(observer.started.lock().unwrap().is_empty());
}

#[tokio::test]
async fn attribution_failure_prevents_the_request_and_settlement_failure_cannot_report_success() {
    for fail_start in [true, false] {
        let server = server(vec![chat("real terminal")]).await;
        let model = model("openai-completions", "fixture", server.base_url());
        let observer = Arc::new(Observer { fail_start, fail_settle: !fail_start, ..Default::default() });
        let auth = Arc::new(FixedRequestAuth::new(RequestAuthLease::new(CredentialIdentity::Runtime, None)));
        let provider = PreparedRoute::new(model.clone(), protocol(&model.api), auth, 0)
            .unwrap()
            .bind(reqwest::Client::new(), Some(observer));
        let result = collect(provider.stream(&model, &context(), CallOptions::default())).await;
        assert_eq!(result.stop_reason, StopReason::Error);
        assert!(result.error_message.as_deref().unwrap().contains("persistence failed"));
        assert_eq!(server.served(), usize::from(!fail_start));
    }
}

#[tokio::test]
async fn changing_only_the_model_is_rejected_before_resolving_or_sending_credentials() {
    let server = server(vec![chat("unexpected")]).await;
    let prepared = model("openai-completions", "fixture", server.base_url());
    let auth = Arc::new(ChangingAuth(AtomicUsize::new(0)));
    let provider = PreparedRoute::new(prepared.clone(), protocol(&prepared.api), auth.clone(), 0)
        .unwrap()
        .bind(reqwest::Client::new(), None);
    let mut changed = prepared;
    changed.api = "anthropic-messages".into();
    let result = collect(provider.stream(&changed, &context(), CallOptions::default())).await;
    assert_eq!(result.stop_reason, StopReason::Error);
    assert_eq!(auth.0.load(Ordering::SeqCst), 0);
    assert_eq!(server.served(), 0);
}

// Original auth-retry.ts families grouped through actual Rust adapters. The
// receipt observer and private lease together identify every outbound attempt.
#[tokio::test]
async fn native_auth_refresh_rotation_cycles_quota_and_wire_retry_family() {
    use std::collections::VecDeque;
    struct Auth {
        initial: RequestAuthLease,
        replies: Mutex<VecDeque<RequestAuthLease>>,
        calls: Mutex<Vec<(CredentialIdentity, AuthRetryAction, RequestAuthFailureKind)>>,
        resolutions: AtomicUsize,
    }
    #[async_trait]
    impl RequestAuthResolver for Auth {
        async fn resolve(&self, _: &Model, _: &CancellationToken) -> Result<RequestAuthLease, AuthResolveError> {
            self.resolutions.fetch_add(1, Ordering::SeqCst);
            Ok(self.initial.clone())
        }
        fn supports_auth_retry(&self) -> bool {
            true
        }
        async fn retry(
            &self,
            _: &Model,
            failed: &RequestAuthLease,
            failure: &RequestAuthFailure,
            action: AuthRetryAction,
            _: &CancellationToken,
        ) -> Result<Option<RequestAuthLease>, AuthResolveError> {
            self.calls.lock().unwrap().push((failed.identity().clone(), action, failure.kind));
            Ok(self.replies.lock().unwrap().pop_front())
        }
    }
    let lease =
        |id, revision, key: &str| RequestAuthLease::new(CredentialIdentity::Stored { id, revision }, Some(key.into()));
    let reject = |status, text: &str| json!({"status":status,"body":json!({"error":{"message":text}}).to_string()});
    struct Case {
        name: &'static str,
        responses: Vec<Value>,
        next: Vec<RequestAuthLease>,
        attempts: usize,
        actions: Vec<(i64, AuthRetryAction, RequestAuthFailureKind)>,
        end: StopReason,
    }
    use AuthRetryAction::{RefreshSame, RotateSibling};
    use RequestAuthFailureKind::{Auth as HardAuth, Forbidden, Quota};
    let mut cap_responses = vec![reject(403, "Forbidden"); 64];
    cap_responses.push(chat("must not be reached"));
    let cap_next = (2..=65).map(|id| lease(id, 0, &format!("synthetic-key-{id}"))).collect();
    let cap_actions = (1..64).map(|id| (id, RotateSibling, Forbidden)).collect();
    for case in [
        Case {
            name: "401 refresh then one sibling",
            responses: vec![
                reject(401, "Unauthorized"),
                reject(401, "Unauthorized"),
                reject(401, "Unauthorized"),
                chat("unreachable"),
            ],
            next: vec![lease(1, 1, "BB"), lease(2, 0, "CCC"), lease(3, 0, "DDDD")],
            attempts: 3,
            actions: vec![(1, RefreshSame, HardAuth), (1, RotateSibling, HardAuth)],
            end: StopReason::Error,
        },
        Case {
            name: "403 traverses siblings",
            responses: vec![reject(403, "Forbidden"), reject(403, "Forbidden"), chat("done")],
            next: vec![lease(2, 0, "BB"), lease(3, 0, "CCC")],
            attempts: 3,
            actions: vec![(1, RotateSibling, Forbidden), (2, RotateSibling, Forbidden)],
            end: StopReason::Stop,
        },
        Case {
            name: "bearer cycle",
            responses: vec![reject(403, "Forbidden"), reject(403, "Forbidden"), chat("unreachable")],
            next: vec![lease(2, 0, "BB"), lease(1, 1, "A")],
            attempts: 2,
            actions: vec![(1, RotateSibling, Forbidden), (2, RotateSibling, Forbidden)],
            end: StopReason::Error,
        },
        Case {
            name: "peer supplies a new bearer for an earlier row",
            responses: vec![reject(403, "Forbidden"), reject(403, "Forbidden"), chat("done")],
            next: vec![lease(2, 0, "BB"), lease(1, 1, "CCC")],
            attempts: 3,
            actions: vec![(1, RotateSibling, Forbidden), (2, RotateSibling, Forbidden)],
            end: StopReason::Stop,
        },
        Case {
            name: "account quota",
            responses: vec![reject(400, "insufficient_quota: quota exceeded"), chat("done")],
            next: vec![lease(2, 0, "BB")],
            attempts: 2,
            actions: vec![(1, RotateSibling, Quota)],
            end: StopReason::Stop,
        },
        Case {
            name: "transient wire retry retains acquired lease",
            responses: vec![reject(429, "Too many requests per minute"), chat("done")],
            next: vec![],
            attempts: 2,
            actions: vec![],
            end: StopReason::Stop,
        },
        Case {
            name: "concurrency is transient",
            responses: vec![reject(403, "Too many concurrent requests")],
            next: vec![],
            attempts: 1,
            actions: vec![],
            end: StopReason::Error,
        },
        Case {
            name: "64 outbound cap",
            responses: cap_responses,
            next: cap_next,
            attempts: 64,
            actions: cap_actions,
            end: StopReason::Error,
        },
    ] {
        let wire = server(case.responses).await;
        let model = model("openai-completions", "fixture", wire.base_url());
        let auth = Arc::new(Auth {
            initial: lease(1, 0, "A"),
            replies: Mutex::new(case.next.into()),
            calls: Mutex::new(Vec::new()),
            resolutions: AtomicUsize::new(0),
        });
        let observer = Arc::new(Observer::default());
        let provider = PreparedRoute::new(model.clone(), protocol(&model.api), auth.clone(), 0)
            .unwrap()
            .bind(reqwest::Client::new(), Some(observer.clone()));
        let result = collect(provider.stream(&model, &context(), CallOptions::default())).await;
        assert_eq!(result.stop_reason, case.end, "{}", case.name);
        assert_eq!(wire.served(), case.attempts, "{}", case.name);
        assert_eq!(auth.resolutions.load(Ordering::SeqCst), 1, "{}", case.name);
        let actual: Vec<_> = auth
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|(identity, action, kind)| {
                let CredentialIdentity::Stored { id, .. } = identity else { panic!("owned row") };
                (*id, *action, *kind)
            })
            .collect();
        assert_eq!(actual, case.actions, "{}", case.name);
        assert_eq!(observer.started.lock().unwrap().len(), observer.settled.lock().unwrap().len(), "{}", case.name);
        if case.actions.is_empty() && case.attempts > 1 {
            let requests = wire.requests.lock().await;
            assert_eq!(requests[0]["headers"]["authorization"], requests[1]["headers"]["authorization"]);
        }
    }
}

// Actual HTTP adapter requests through the trusted Host reset hook. These
// manually authored receipts prove request admission only; saved-credit
// selection, consume transport and durable fences have their own Host tests.
#[tokio::test]
async fn quota_reset_receipt_sibling_cycle_budget_cancellation_and_replay_veto_family() {
    use ara_cli::codex_reset_receipts::{ResetOperationReceipt, ResetReceiptCode, ResetReceiptState};
    use std::collections::VecDeque;
    use tokio::sync::Semaphore;

    struct Auth {
        initial: RequestAuthLease,
        siblings: Mutex<VecDeque<Option<RequestAuthLease>>>,
        retries: Mutex<Vec<(CredentialIdentity, AuthRetryAction, RequestAuthFailureKind)>>,
        reset_calls: AtomicUsize,
        resolutions: AtomicUsize,
        restored: RequestAuthLease,
        receipt: Option<ResetOperationReceipt>,
        hook_error: bool,
        gate: Option<(Arc<Semaphore>, Arc<Semaphore>)>,
    }
    #[async_trait]
    impl RequestAuthResolver for Auth {
        async fn resolve(&self, _: &Model, _: &CancellationToken) -> Result<RequestAuthLease, AuthResolveError> {
            self.resolutions.fetch_add(1, Ordering::SeqCst);
            Ok(self.initial.clone())
        }
        fn supports_auth_retry(&self) -> bool {
            true
        }
        async fn retry(
            &self,
            _: &Model,
            failed: &RequestAuthLease,
            failure: &RequestAuthFailure,
            action: AuthRetryAction,
            _: &CancellationToken,
        ) -> Result<Option<RequestAuthLease>, AuthResolveError> {
            self.retries.lock().unwrap().push((failed.identity().clone(), action, failure.kind));
            Ok(self.siblings.lock().unwrap().pop_front().flatten())
        }
        async fn quota_reset(
            &self,
            _: &Model,
            _: &RequestAuthLease,
            failure: &RequestAuthFailure,
            _: &CancellationToken,
        ) -> Result<Option<QuotaResetReplay>, AuthResolveError> {
            assert_eq!(failure.kind, RequestAuthFailureKind::Quota);
            self.reset_calls.fetch_add(1, Ordering::SeqCst);
            if let Some((entered, release)) = &self.gate {
                entered.add_permits(1);
                release.acquire().await.unwrap().forget();
            }
            if self.hook_error {
                return Err(AuthResolveError::Refresh);
            }
            Ok(self
                .receipt
                .as_ref()
                .and_then(|receipt| QuotaResetReplay::from_confirmed(self.restored.clone(), receipt)))
        }
    }
    let lease = |id, key: &str| RequestAuthLease::new(CredentialIdentity::Stored { id, revision: 0 }, Some(key.into()));
    let receipt = |state, code| ResetOperationReceipt {
        request_id: uuid::Uuid::new_v4().to_string(),
        account_key: "fixture-account".into(),
        credential_id: 1,
        account_id: Some("fixture-account".into()),
        email: Some("fixture@example.com".into()),
        credit_id: "fixture-credit".into(),
        endpoint_fingerprint: "fixture-endpoint-fingerprint".into(),
        created_at: 1_700_000_040_000.0,
        updated_at: 1_700_000_040_001.0,
        state,
        code,
    };
    let quota = || json!({"status":400,"body":json!({"error":{"code":"insufficient_quota","message":"insufficient_quota: quota exceeded"}}).to_string()});
    let confirmed = || Some(receipt(ResetReceiptState::Known, ResetReceiptCode::Reset));
    struct Case {
        name: &'static str,
        responses: Vec<Value>,
        siblings: Vec<Option<RequestAuthLease>>,
        receipt: Option<ResetOperationReceipt>,
        restored_row: i64,
        hook_error: bool,
        rows: Vec<i64>,
        hooks: usize,
        end: StopReason,
    }
    let mut cases = vec![
        Case {
            name: "usable sibling precedes reset",
            responses: vec![quota(), chat("sibling")],
            siblings: vec![Some(lease(2, "BB"))],
            receipt: confirmed(),
            restored_row: 1,
            hook_error: false,
            rows: vec![1, 2],
            hooks: 0,
            end: StopReason::Stop,
        },
        Case {
            name: "confirmed reset admits the exact restored row with a seen bearer",
            responses: vec![quota(), chat("restored")],
            siblings: vec![],
            receipt: confirmed(),
            restored_row: 1,
            hook_error: false,
            rows: vec![1, 1],
            hooks: 1,
            end: StopReason::Stop,
        },
        Case {
            name: "confirmed same-bearer exception is used once",
            responses: vec![quota(), quota(), chat("unreachable")],
            siblings: vec![],
            receipt: confirmed(),
            restored_row: 1,
            hook_error: false,
            rows: vec![1, 1],
            hooks: 1,
            end: StopReason::Error,
        },
        Case {
            name: "reset preserves earlier seen sibling bearers",
            responses: vec![quota(), quota(), quota(), chat("unreachable")],
            siblings: vec![Some(lease(2, "BB")), None, Some(lease(2, "BB"))],
            receipt: confirmed(),
            restored_row: 1,
            hook_error: false,
            rows: vec![1, 2, 1],
            hooks: 1,
            end: StopReason::Error,
        },
        Case {
            name: "ordinary repeated bearer is not a reset acknowledgement",
            responses: vec![quota(), chat("unreachable")],
            siblings: vec![Some(lease(1, "A"))],
            receipt: None,
            restored_row: 1,
            hook_error: false,
            rows: vec![1],
            hooks: 1,
            end: StopReason::Error,
        },
        Case {
            name: "hook error retains the original provider quota rejection",
            responses: vec![quota(), chat("unreachable")],
            siblings: vec![],
            receipt: confirmed(),
            restored_row: 1,
            hook_error: true,
            rows: vec![1],
            hooks: 1,
            end: StopReason::Error,
        },
        Case {
            name: "wrong returned row cannot use a seen bearer",
            responses: vec![quota(), chat("unreachable")],
            siblings: vec![],
            receipt: confirmed(),
            restored_row: 2,
            hook_error: false,
            rows: vec![1],
            hooks: 1,
            end: StopReason::Error,
        },
    ];
    for (name, state, code) in [
        ("already_redeemed is not a confirmed reset", ResetReceiptState::Known, ResetReceiptCode::AlreadyRedeemed),
        ("no_credit is not a confirmed reset", ResetReceiptState::Known, ResetReceiptCode::NoCredit),
        ("nothing_to_reset is not a confirmed reset", ResetReceiptState::Known, ResetReceiptCode::NothingToReset),
        ("Unknown cannot admit a replay", ResetReceiptState::Unknown, ResetReceiptCode::Unknown),
        ("Pending cannot admit a replay", ResetReceiptState::Pending, ResetReceiptCode::Pending),
    ] {
        cases.push(Case {
            name,
            responses: vec![quota(), chat("unreachable")],
            siblings: vec![],
            receipt: Some(receipt(state, code)),
            restored_row: 1,
            hook_error: false,
            rows: vec![1],
            hooks: 1,
            end: StopReason::Error,
        });
    }
    let mut wrong_receipt = receipt(ResetReceiptState::Known, ResetReceiptCode::Reset);
    wrong_receipt.credential_id = 2;
    cases.push(Case {
        name: "wrong confirmed receipt row cannot use a seen bearer",
        responses: vec![quota(), chat("unreachable")],
        siblings: vec![],
        receipt: Some(wrong_receipt),
        restored_row: 1,
        hook_error: false,
        rows: vec![1],
        hooks: 1,
        end: StopReason::Error,
    });
    let mut invalid_uuid = receipt(ResetReceiptState::Known, ResetReceiptCode::Reset);
    invalid_uuid.request_id = "not-a-request-uuid".into();
    cases.push(Case {
        name: "missing request identity is not confirmation",
        responses: vec![quota(), chat("unreachable")],
        siblings: vec![],
        receipt: Some(invalid_uuid),
        restored_row: 1,
        hook_error: false,
        rows: vec![1],
        hooks: 1,
        end: StopReason::Error,
    });
    let mut capped = (0..64).map(|_| quota()).collect::<Vec<_>>();
    capped.push(chat("unreachable"));
    cases.push(Case {
        name: "MAX64 exhaustion never calls reset",
        responses: capped.clone(),
        siblings: (2..=65).map(|id| Some(lease(id, &format!("synthetic-key-{id}")))).collect(),
        receipt: confirmed(),
        restored_row: 1,
        hook_error: false,
        rows: (1..=64).collect(),
        hooks: 0,
        end: StopReason::Error,
    });
    let reset_then_siblings =
        std::iter::once(None).chain((2..=64).map(|id| Some(lease(id, &format!("synthetic-key-{id}"))))).collect();
    let reset_budget_rows = [1, 1].into_iter().chain(2..=63).collect();
    cases.push(Case {
        name: "confirmed reset consumes the existing MAX64 budget",
        responses: capped,
        siblings: reset_then_siblings,
        receipt: confirmed(),
        restored_row: 1,
        hook_error: false,
        rows: reset_budget_rows,
        hooks: 1,
        end: StopReason::Error,
    });
    for case in cases {
        let wire = server(case.responses).await;
        let model = model("openai-completions", "openai-codex", wire.base_url());
        let auth = Arc::new(Auth {
            initial: lease(1, "A"),
            siblings: Mutex::new(case.siblings.into()),
            retries: Mutex::new(Vec::new()),
            reset_calls: AtomicUsize::new(0),
            resolutions: AtomicUsize::new(0),
            restored: lease(case.restored_row, "A"),
            receipt: case.receipt,
            hook_error: case.hook_error,
            gate: None,
        });
        let observer = Arc::new(Observer::default());
        let provider = PreparedRoute::new(model.clone(), protocol(&model.api), auth.clone(), 43)
            .unwrap()
            .bind(reqwest::Client::new(), Some(observer.clone()));
        let result = collect(provider.stream(&model, &context(), CallOptions::default())).await;
        assert_eq!(result.stop_reason, case.end, "{}", case.name);
        assert_eq!(wire.served(), case.rows.len(), "{}", case.name);
        assert_eq!(auth.resolutions.load(Ordering::SeqCst), 1, "{}", case.name);
        assert_eq!(auth.reset_calls.load(Ordering::SeqCst), case.hooks, "{}", case.name);
        let started = observer.started.lock().unwrap().clone();
        let rows = started
            .iter()
            .map(|request| match request.credential {
                CredentialIdentity::Stored { id, .. } => id,
                _ => panic!("stored Host row"),
            })
            .collect::<Vec<_>>();
        assert_eq!(rows, case.rows, "{}", case.name);
        assert_eq!(started.len(), observer.settled.lock().unwrap().len(), "{}", case.name);
        assert!(
            auth.retries
                .lock()
                .unwrap()
                .iter()
                .all(|(_, action, kind)| *action == AuthRetryAction::RotateSibling
                    && *kind == RequestAuthFailureKind::Quota),
            "{}",
            case.name
        );
        let requests = wire.requests.lock().await;
        for request in requests.iter() {
            assert_eq!(request["body"], requests[0]["body"], "{}", case.name);
        }
        if case.name == "confirmed reset admits the exact restored row with a seen bearer" {
            assert_eq!(requests[0]["headers"]["authorization"], requests[1]["headers"]["authorization"]);
            assert_ne!(started[0].call_id, started[1].call_id);
        }
        if result.stop_reason == StopReason::Error {
            assert_eq!(result.error_status, Some(400), "{}", case.name);
            assert!(result.error_message.as_deref().unwrap().contains("quota exceeded"), "{}", case.name);
            let evidence = result.failure_evidence.as_ref().unwrap();
            assert_eq!(evidence.kind, ara_ai::retry_classification::ProviderErrorKind::Http);
            assert!(
                evidence.replay_blocked && evidence.same_route_blocked,
                "real Completions UsageAdmission facts remain intact"
            );
            assert_eq!(evidence.context_recovery, Some(ara_ai::ContextRecoveryEvidence::UsageAdmission));
        }
    }

    // A Host consume already started during cancellation must settle, then the
    // caller receives Aborted. Even a confirmed receipt cannot dispatch again.
    let wire = server(vec![quota(), chat("must not dispatch after cancellation")]).await;
    let prepared_model = model("openai-completions", "openai-codex", wire.base_url());
    let entered = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    let auth = Arc::new(Auth {
        initial: lease(1, "A"),
        siblings: Mutex::new(VecDeque::new()),
        retries: Mutex::new(Vec::new()),
        reset_calls: AtomicUsize::new(0),
        resolutions: AtomicUsize::new(0),
        restored: lease(1, "A"),
        receipt: confirmed(),
        hook_error: false,
        gate: Some((entered.clone(), release.clone())),
    });
    let observer = Arc::new(Observer::default());
    let provider = PreparedRoute::new(prepared_model.clone(), protocol(&prepared_model.api), auth.clone(), 43)
        .unwrap()
        .bind(reqwest::Client::new(), Some(observer.clone()));
    let caller = CancellationToken::new();
    let stream =
        provider.stream(&prepared_model, &context(), CallOptions { cancel: caller.clone(), ..Default::default() });
    tokio::time::timeout(Duration::from_secs(2), entered.acquire()).await.unwrap().unwrap().forget();
    caller.cancel();
    release.add_permits(1);
    assert_eq!(collect(stream).await.stop_reason, StopReason::Aborted);
    assert_eq!(auth.reset_calls.load(Ordering::SeqCst), 1);
    assert_eq!(wire.served(), 1);
    assert_eq!(observer.started.lock().unwrap().len(), 1);

    // Visible text/tool fragments, an untyped stream failure and a failed
    // post-success callback each veto the hook before any confirmation matters.
    for (name, scripted, transport_callback, fail_settle) in [
        (
            "visible text then quota",
            json!({"events":[{"data":{"choices":[{"delta":{"content":"partial retained"}}]}},{"data":{"error":{"code":400,"message":"insufficient_quota: quota exceeded"}}}]}),
            false,
            false,
        ),
        (
            "partial tool then quota",
            json!({"events":[{"data":{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_partial","type":"function","function":{"name":"write_fixture","arguments":"{"}}]}}]}},{"data":{"error":{"code":400,"message":"insufficient_quota: quota exceeded"}}}]}),
            false,
            false,
        ),
        (
            "statusless quota text does not prove a rejected POST",
            json!({"events":[{"data":{"error":"usage limit exceeded; effect unknown"}}]}),
            false,
            false,
        ),
        ("unknown successful-POST callback effect", chat("unused"), true, false),
        ("failed local settlement veto", quota(), false, true),
    ] {
        let wire = server(vec![scripted, chat("unreachable replay")]).await;
        let model = model("openai-completions", "openai-codex", wire.base_url());
        let auth = Arc::new(Auth {
            initial: lease(1, "A"),
            siblings: Mutex::new(VecDeque::new()),
            retries: Mutex::new(Vec::new()),
            reset_calls: AtomicUsize::new(0),
            resolutions: AtomicUsize::new(0),
            restored: lease(1, "A"),
            receipt: confirmed(),
            hook_error: false,
            gate: None,
        });
        let observer = Arc::new(Observer { fail_settle, ..Default::default() });
        let provider = PreparedRoute::new(model.clone(), protocol(&model.api), auth.clone(), 43)
            .unwrap()
            .bind(reqwest::Client::new(), Some(observer.clone()));
        let callback = transport_callback.then(|| {
            ara_ai::ProviderResponseCallback::new(|_, _| {
                Box::pin(async {
                    Err(ara_ai::ProviderError::Transport("usage limit text; successful POST outcome unknown".into()))
                })
            })
        });
        let result =
            collect(provider.stream(&model, &context(), CallOptions { on_response: callback, ..Default::default() }))
                .await;
        assert_eq!(result.stop_reason, StopReason::Error, "{name}");
        assert_eq!(wire.served(), 1, "{name}");
        assert_eq!(auth.reset_calls.load(Ordering::SeqCst), 0, "{name}");
        assert!(auth.retries.lock().unwrap().is_empty(), "{name}");
        assert_eq!(observer.started.lock().unwrap().len(), 1, "{name}");
        if name == "visible text then quota" {
            assert_eq!(result.text(), "partial retained");
        }
        if name == "partial tool then quota" {
            assert!(!result.content.is_empty());
        }
        if transport_callback {
            // notify_provider_response turns any callback failure following a
            // successful POST into Config; callback text is not wire evidence.
            let evidence = result.failure_evidence.unwrap();
            assert_eq!(evidence.kind, ara_ai::retry_classification::ProviderErrorKind::Config);
            assert!(evidence.replay_blocked && evidence.same_route_blocked);
        }
    }
}

#[tokio::test]
async fn configured_key_suppresses_oauth_selection_and_account_identity() {
    use ara_cli::config_request_auth::{ConfigRequestAuth, ConfigRequestAuthSpec};
    struct Account(AtomicUsize);
    #[async_trait]
    impl RequestAuthResolver for Account {
        async fn resolve(&self, _: &Model, _: &CancellationToken) -> Result<RequestAuthLease, AuthResolveError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(AuthResolveError::Refresh)
        }
    }
    let wire = server(vec![chat("configured")]).await;
    let model = model("openai-completions", "fixture", wire.base_url());
    let account = Arc::new(Account(AtomicUsize::new(0)));
    let auth = Arc::new(ConfigRequestAuth::new(
        ConfigRequestAuthSpec {
            base: RequestAuthLease::new(CredentialIdentity::Keyless, None),
            key_config: Some("synthetic-config-key".into()),
            startup_key: None,
            header_sources: Vec::new(),
            composed_headers: None,
            invalidation_values: Vec::new(),
            cli_headers: Vec::new(),
            auth_header: false,
            codex_account: true,
        },
        std::env::current_dir().unwrap(),
        Some(account.clone()),
    ));
    let observer = Arc::new(Observer::default());
    let provider = PreparedRoute::new(model.clone(), protocol(&model.api), auth, 0)
        .unwrap()
        .bind(reqwest::Client::new(), Some(observer.clone()));
    assert_eq!(collect(provider.stream(&model, &context(), CallOptions::default())).await.text(), "configured");
    assert_eq!(account.0.load(Ordering::SeqCst), 0);
    assert_eq!(
        observer.started.lock().unwrap()[0].credential,
        CredentialIdentity::Config { provider: "fixture".into() }
    );
    assert_eq!(wire.served(), 1);
    assert!(wire.requests.lock().await[0]["headers"].get("chatgpt-account-id").is_none());
}

#[test]
fn route_preparation_rejects_stale_protocol_or_static_credentials() {
    let model = model("openai-completions", "fixture", "http://localhost/v1".into());
    let auth = Arc::new(FixedRequestAuth::new(RequestAuthLease::new(CredentialIdentity::Runtime, None)));
    assert!(matches!(
        PreparedRoute::new(model.clone(), protocol("anthropic-messages"), auth.clone(), 0),
        Err(RoutePrepareError::ProtocolMismatch)
    ));
    let options = ProtocolOptions::Completions(ara_ai::providers::openai_completions::StreamOptions {
        api_key: Some("unused".into()),
        ..Default::default()
    });
    assert!(matches!(
        PreparedRoute::new(model.clone(), options, auth.clone(), 0),
        Err(RoutePrepareError::StaticCredential)
    ));
    for header in ["Authorization", "X-Api-Key", "X-Goog-Api-Key", "pRoXy-AuThOrIzAtIoN"] {
        let options = ProtocolOptions::Completions(ara_ai::providers::openai_completions::StreamOptions {
            extra_headers: vec![(header.into(), "unused".into())],
            ..Default::default()
        });
        assert!(matches!(
            PreparedRoute::new(model.clone(), options, auth.clone(), 0),
            Err(RoutePrepareError::StaticCredential)
        ));
    }
}

#[tokio::test]
async fn dropped_consumer_during_auth_releases_the_pending_resolver() {
    struct Guard(Arc<AtomicUsize>);
    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    struct GuardedAuth {
        notify: Arc<tokio::sync::Notify>,
        dropped: Arc<AtomicUsize>,
    }
    #[async_trait]
    impl RequestAuthResolver for GuardedAuth {
        async fn resolve(&self, _: &Model, _: &CancellationToken) -> Result<RequestAuthLease, AuthResolveError> {
            let _guard = Guard(self.dropped.clone());
            self.notify.notify_one();
            std::future::pending().await
        }
    }
    let server = server(vec![chat("unexpected")]).await;
    let model = model("openai-completions", "fixture", server.base_url());
    let notify = Arc::new(tokio::sync::Notify::new());
    let dropped = Arc::new(AtomicUsize::new(0));
    let auth = Arc::new(GuardedAuth { notify: notify.clone(), dropped: dropped.clone() });
    let provider =
        PreparedRoute::new(model.clone(), protocol(&model.api), auth, 0).unwrap().bind(reqwest::Client::new(), None);
    let stream = provider.stream(&model, &context(), CallOptions::default());
    tokio::time::timeout(Duration::from_secs(2), notify.notified()).await.unwrap();
    drop(stream);
    tokio::time::timeout(Duration::from_secs(2), async {
        while dropped.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(server.served(), 0);
}

#[tokio::test]
async fn live_snapshot_adopts_the_whole_route_on_the_next_call_of_the_same_agent() {
    use ara_agent::{
        Agent, AgentConfig, AgentTool, ExecutionSnapshot, LoopHooks, NoHooks, NullSink, RunEnd, ToolError, ToolOutput,
        UpdateFn,
    };
    use std::sync::RwLock;
    struct Hooks(RwLock<ExecutionSnapshot>);
    #[async_trait]
    impl LoopHooks for Hooks {
        fn execution_snapshot(&self) -> Option<ExecutionSnapshot> {
            Some(self.0.read().unwrap().clone())
        }
    }
    struct WriteFixture {
        path: std::path::PathBuf,
        marker: &'static str,
    }
    #[async_trait]
    impl AgentTool for WriteFixture {
        fn definition(&self) -> ara_ai::Tool {
            ara_ai::Tool {
                name: "write_fixture".into(),
                description: self.marker.into(),
                parameters: json!({"type":"object","properties":{}}),
            }
        }
        async fn execute(
            &self,
            _: &str,
            _: ara_ai::JsonObject,
            _: CancellationToken,
            _: UpdateFn,
        ) -> Result<ToolOutput, ToolError> {
            std::fs::write(&self.path, self.marker)?;
            Ok(ToolOutput::text(self.marker))
        }
    }
    let a_reply = json!({"delay_ms":150,"events":[
        {"data":{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_A","type":"function","function":{"name":"write_fixture","arguments":"{}"}}]}}]}},
        {"data":{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}},{"done":true}
    ]});
    let a_server = server(vec![a_reply]).await;
    let b_server = server(vec![responses("continued on B"), responses("queued followup on B")]).await;
    let a_model = model("openai-completions", "A", a_server.base_url());
    let b_model = model("openai-responses", "B", b_server.base_url());
    let make_provider = |model: &Model, key: &str, generation| {
        PreparedRoute::new(
            model.clone(),
            protocol(&model.api),
            Arc::new(FixedRequestAuth::new(RequestAuthLease::new(CredentialIdentity::Runtime, Some(key.into())))),
            generation,
        )
        .unwrap()
        .bind(reqwest::Client::new(), None)
    };
    let dir = tempfile::tempdir().unwrap();
    let artifact = dir.path().join("snapshot.txt");
    let a_config = AgentConfig {
        model: a_model.clone(),
        provider: make_provider(&a_model, "a-value", 1),
        system_prompt: vec!["prompt A".into()],
        tools: vec![Arc::new(WriteFixture { path: artifact.clone(), marker: "tool A retained" })],
        tool_choice: None,
        max_tokens: Some(100),
        temperature: Some(0.1),
        deadline: None,
        max_model_calls: Some(3),
        hooks: Arc::new(NoHooks),
    };
    let mut b_config = a_config.clone();
    b_config.model = b_model.clone();
    b_config.provider = make_provider(&b_model, "b-longer-value", 2);
    b_config.system_prompt = vec!["prompt B".into()];
    b_config.tools = vec![Arc::new(WriteFixture { path: artifact.clone(), marker: "wrong tool B" })];
    b_config.max_tokens = Some(200);
    b_config.temperature = Some(0.2);
    let hooks = Arc::new(Hooks(RwLock::new(ExecutionSnapshot::from_config(&a_config))));
    let mut config = a_config;
    config.hooks = hooks.clone();
    let agent = Agent::new(config, Vec::new());
    let owner = agent.clone();
    let running = tokio::spawn(async move {
        owner
            .prompt(vec![Message::User(UserMessage::text("start"))], CancellationToken::new(), Arc::new(NullSink))
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while a_server.served() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(agent.is_busy());
    agent.follow_up(Message::User(UserMessage::text("queued before switch")));
    *hooks.0.write().unwrap() = ExecutionSnapshot::from_config(&b_config);
    let report = tokio::time::timeout(Duration::from_secs(5), running).await.unwrap().unwrap().unwrap();
    assert_eq!(report.end, RunEnd::Completed);
    assert!(!agent.is_busy());
    assert_eq!(agent.queued_counts(), (0, 0));
    assert_eq!(std::fs::read_to_string(&artifact).unwrap(), "tool A retained");
    assert_eq!(a_server.served(), 1);
    let b_requests = b_server.requests.lock().await;
    assert_eq!(b_requests.len(), 2);
    assert_eq!(b_requests[0]["request"], "POST /v1/responses HTTP/1.1");
    assert_eq!(b_requests[0]["body"]["model"], b_model.id);
    assert_eq!(b_requests[0]["body"]["instructions"], "prompt B");
    assert_eq!(b_requests[0]["body"]["max_output_tokens"], 200);
    assert_eq!(b_requests[0]["body"]["temperature"], 0.2);
    assert_eq!(b_requests[0]["headers"]["authorization"], "<redacted 21 chars>");
    assert!(
        b_requests[0]["body"]["input"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["type"] == "function_call_output" && item["output"] == "tool A retained")
    );
    assert!(b_requests[1]["body"].to_string().contains("queued before switch"));
    let messages = agent.messages().await;
    assert!(messages.iter().any(|message| matches!(message, Message::Assistant(message) if message.provider == "A")));
    assert_eq!(
        messages
            .iter()
            .filter(|message| matches!(message, Message::Assistant(message) if message.provider == "B"))
            .count(),
        2
    );
}

#[tokio::test]
async fn settlement_failure_retains_the_provider_content_usage_and_native_id() {
    let mut reply = responses("visible evidence");
    reply["events"][1]["data"]["response"]["usage"] = json!({"input_tokens":7,"output_tokens":3,"total_tokens":10});
    let server = server(vec![reply]).await;
    let model = model("openai-responses", "fixture", server.base_url());
    let observer = Arc::new(Observer { fail_settle: true, ..Default::default() });
    let auth = Arc::new(FixedRequestAuth::new(RequestAuthLease::new(CredentialIdentity::Runtime, None)));
    let provider = PreparedRoute::new(model.clone(), protocol(&model.api), auth, 0)
        .unwrap()
        .bind(reqwest::Client::new(), Some(observer));
    let result = collect(provider.stream(&model, &context(), CallOptions::default())).await;
    assert_eq!(result.stop_reason, StopReason::Error);
    assert_eq!(result.text(), "visible evidence");
    assert_eq!(result.response_id.as_deref(), Some("resp_fixture"));
    assert_eq!(result.usage.input, Some(7));
    assert_eq!(result.usage.output, Some(3));
    assert_eq!(result.usage.total_tokens, Some(10));
    assert!(result.failure_evidence.as_ref().unwrap().same_route_blocked);
}
