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

fn model(api: &str, provider: &str, base_url: String) -> Model {
    Model {
        id: format!("{provider}-model"),
        api: api.into(),
        provider: provider.into(),
        base_url,
        reasoning: false,
        max_tokens: None,
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
