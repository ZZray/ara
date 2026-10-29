//! Shared async observations called by one existing main.rs coarse entrance.
//! No new test attribute; this is a controlled Core/SQLite fault family.
use super::*;
use ara_agent::{
    ActualProviderContext, Agent, AgentEvent, AgentEventSink, AgentMessageBody, OwnedProviderContextPort,
    ProviderContextBinding, ProviderContextError,
    agent_loop::{AgentConfig, InterruptMode, LoopHooks},
};
use ara_ai::{
    ActualAssistantEvent, ActualAssistantStream, AssistantStream, CallOptions, Context, ModelProvider, StopReason,
};
use ara_cli::{
    auth_storage::AuthStorageOptions,
    credential_store::{AuthCredential, SqliteCredentialStore},
};
use ara_rpc::value_heap::{NoRuntime, ValueHeap, ValueSnapshot};
use async_trait::async_trait;

fn own(values: &SessionValues, root: &ValueHandle, key: &str, value: ValueHandle) {
    values.heap().define_own_data(root, key.into(), value, values.runtime()).unwrap();
}
fn failure(values: &SessionValues, model: &Model, known: bool) -> ValueHandle {
    let root = values.heap().object().unwrap();
    for (key, value) in [
        ("role", "assistant"),
        ("api", model.api.as_str()),
        ("provider", model.provider.as_str()),
        ("model", model.id.as_str()),
        ("stopReason", "error"),
        ("errorMessage", if known { "usage_limit_reached retry-after-ms=90000" } else { "dispatch outcome unknown" }),
    ] {
        own(values, &root, key, values.heap().string(value.into()).unwrap());
    }
    own(values, &root, "timestamp", values.heap().number(7.0).unwrap());
    own(values, &root, "content", values.heap().array(vec![]).unwrap());
    own(values, &root, "usage", values.heap().object().unwrap());
    let facts = values.heap().object().unwrap();
    own(values, &facts, "kind", values.heap().string(if known { "http" } else { "transport" }.into()).unwrap());
    if known {
        own(values, &facts, "code", values.heap().string("usage_limit_reached".into()).unwrap());
        own(values, &facts, "status", values.heap().number(429.0).unwrap());
        own(values, &facts, "contextRecovery", values.heap().string("usageAdmission".into()).unwrap());
        own(values, &facts, "waitMs", values.heap().number(90_000.0).unwrap());
    }
    own(
        values,
        &facts,
        "errorId",
        values
            .heap()
            .number(if known {
                f64::from(ara_ai::retry_classification::flag::CLASS | ara_ai::retry_classification::flag::USAGE_LIMIT)
            } else {
                0.0
            })
            .unwrap(),
    );
    own(values, &facts, "replayBlocked", values.heap().boolean(true).unwrap());
    own(values, &facts, "sameRouteBlocked", values.heap().boolean(true).unwrap());
    own(values, &root, "failureEvidence", facts);
    root
}
struct NeverTyped;
impl ModelProvider for NeverTyped {
    fn stream(&self, _: &Model, _: &Context, _: CallOptions) -> AssistantStream {
        panic!("quota actual fixture reached typed provider")
    }
}
struct TerminalPort {
    terminal: ValueHandle,
}
impl OwnedProviderContextPort for TerminalPort {
    fn project_context(&self, _: &Model, _: Context, _: bool) -> Result<Context, ProviderContextError> {
        panic!("quota fixture typed projection")
    }
    fn stream(&self, _: &dyn ModelProvider, _: &Model, _: &Context, _: CallOptions) -> AssistantStream {
        panic!("quota fixture typed stream")
    }
    fn project_context_actual(
        &self,
        _: &Model,
        context: ActualProviderContext,
        _: bool,
        values: &SessionValues,
    ) -> Result<ActualProviderContext, ActualProviderError> {
        context.check_lineage(values)?;
        Ok(context)
    }
    fn stream_prepared_original_actual(
        &self,
        _: &dyn ModelProvider,
        _: &Model,
        context: &ActualProviderContext,
        _: Option<ValueHandle>,
        values: Arc<SessionValues>,
        _: CallOptions,
    ) -> Result<ActualAssistantStream, ActualProviderError> {
        context.check_lineage(&values)?;
        values.check_handle(&self.terminal)?;
        let (sink, stream, receipt) = ActualAssistantStream::channel();
        let terminal = self.terminal.clone();
        let settled = receipt.owner_settlement_guard();
        tokio::spawn(async move {
            let _settled = settled;
            let event = ActualAssistantEvent::Error { reason: StopReason::Error, error: terminal };
            receipt.retain_terminal(&event);
            let _ = sink.push(event).await;
        });
        Ok(stream)
    }
}
struct Hooks {
    binding: ProviderContextBinding,
}
#[async_trait]
impl LoopHooks for Hooks {
    fn provider_context_binding(&self) -> Option<ProviderContextBinding> {
        Some(self.binding.clone())
    }
    async fn convert_agent_messages_to_llm_actual(
        &self,
        messages: &[AgentMessage],
        values: &SessionValues,
    ) -> Result<Vec<ValueHandle>, ActualProviderError> {
        messages
            .iter()
            .map(|message| match message.body_snapshot() {
                AgentMessageBody::Graph(root) => {
                    values.check_handle(&root)?;
                    Ok(root)
                }
                _ => Err(ActualProviderError::Unsupported("quota fixture Graph history")),
            })
            .collect()
    }
    async fn transform_provider_context_actual(
        &self,
        context: ActualProviderContext,
        _: &Model,
        values: &SessionValues,
    ) -> Result<ActualProviderContext, ActualProviderError> {
        context.check_lineage(values)?;
        Ok(context)
    }
    async fn transform_assistant_graph(
        &self,
        _: &AgentMessage,
        _: &CancellationToken,
        _: &SessionValues,
    ) -> Result<(), ActualProviderError> {
        Ok(())
    }
    fn supports_actual_turn_end(&self) -> bool {
        true
    }
    async fn turn_end_actual(
        &self,
        _: &AgentMessage,
        _: &[AgentMessage],
        _: &CancellationToken,
        _: &SessionValues,
    ) -> Result<Vec<AgentMessage>, ActualProviderError> {
        Ok(vec![])
    }
}
struct Sink;
#[async_trait]
impl AgentEventSink for Sink {
    async fn emit(&self, _: AgentEvent) {}
}

async fn canonical(values: Arc<SessionValues>, model: &Model, terminal: ValueHandle) -> AgentMessage {
    let binding = ProviderContextBinding {
        actual_values: Some(values.clone()),
        owned_dialect: true,
        owned_port: Some(Arc::new(TerminalPort { terminal })),
        ..Default::default()
    };
    let config = AgentConfig {
        model: model.clone(),
        provider: Arc::new(NeverTyped),
        system_prompt: vec![],
        tools: vec![],
        tool_choice: None,
        max_tokens: None,
        temperature: None,
        deadline: None,
        max_model_calls: Some(1),
        hooks: Arc::new(Hooks { binding }),
        interrupt_mode: InterruptMode::Immediate,
    };
    let agent = Agent::new_raw(config, vec![]).unwrap();
    let user = values.heap().object().unwrap();
    own(&values, &user, "role", values.heap().string("user".into()).unwrap());
    own(&values, &user, "content", values.heap().string("bounded quota observation".into()).unwrap());
    own(&values, &user, "timestamp", values.heap().number(6.0).unwrap());
    let report = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        agent.prompt_raw(vec![AgentMessage::from_graph(user, None).unwrap()], CancellationToken::new(), Arc::new(Sink)),
    )
    .await
    .expect("bounded actual Core terminal")
    .unwrap();
    report.messages.into_iter().find(|message| match message.body_snapshot() {
        AgentMessageBody::Graph(root) => matches!(values.heap().snapshot(&values.heap().get(&root, &"role".into(), values.runtime()).unwrap()).unwrap(), ValueSnapshot::String(role) if role.equals_ascii("assistant")),
        _ => false,
    }).expect("actual Core retained terminal")
}

/// This exercises the actual observer, independent feedback owner, Temp SQLite
/// block and Core-signed original terminal. Driver queue/dispatch exclusion is
/// observed separately in Root's existing model_route coarse driver entrance.
pub(crate) async fn stored_feedback_family() {
    for scenario in [
        "once",
        "known-cancel",
        "unknown",
        "logical",
        "model",
        "lease",
        "epoch",
        "closing",
        "foreign",
        "runtime",
        "public-forgery",
        "replacement",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let model = Model {
            id: "quota-native-fixture".into(),
            api: "openai-completions".into(),
            provider: "quota-native-fixture".into(),
            base_url: String::new(),
            reasoning: false,
            max_tokens: None,
            context_window: None,
            tokenizer: None,
        };
        let store = Arc::new(Mutex::new(SqliteCredentialStore::open(dir.path().join("credentials.db")).unwrap()));
        let rows = store
            .lock()
            .unwrap()
            .upsert_auth_credential_for_provider(&model.provider, &AuthCredential::api_key("synthetic-quota-key"))
            .unwrap();
        let row_id = rows[0].id;
        let account = Arc::new(AuthStorage::new(store.clone(), None, AuthStorageOptions::default()).unwrap());
        let session = "quota-logical-fixture".to_owned();
        let host =
            Arc::new(super::super::ReferenceResetHost::new(dir.path().to_path_buf(), &model, Some(session.clone())));
        let context = AuthRequestContext {
            session_id: Some(session.clone()),
            model_id: Some(model.id.clone()),
            ..Default::default()
        };
        let resolver = account.session_resolver(Some(session.clone()), None);
        let lease = resolver.resolve(&model, &CancellationToken::new()).await.unwrap();
        let binding = QuotaBinding {
            model: model.clone(),
            generation: 17,
            logical_session: Some(session.clone()),
            context,
            reset_context: CodexResetContext::for_model(&model, Some(session.clone()), None),
            host: host.clone(),
            account: account.clone(),
            resolver,
        };
        let owner = Arc::new(SessionQuotaRecovery::default());
        let observer = owner.observer(binding);
        let request = RequestIdentity {
            call_id: uuid::Uuid::new_v4(),
            provider: model.provider.clone(),
            model_id: model.id.clone(),
            route_generation: 17,
            credential: lease.identity().clone(),
        };
        observer.request_auth(request.call_id).unwrap();
        observer.started(&request).unwrap();
        let values = Arc::new(SessionValues::new(ValueHeap::new().unwrap(), Arc::new(NoRuntime)).unwrap());
        let terminal = failure(&values, &model, scenario != "unknown");
        observer.settled_actual(&request, &terminal, values.factory()).unwrap();
        if scenario == "closing" {
            owner.settle().await;
        }
        let rejected_lease = if scenario == "lease" {
            RequestAuthLease::new(
                CredentialIdentity::Stored { id: row_id + 1, revision: 999 },
                Some("foreign-key".into()),
            )
        } else {
            lease.clone()
        };
        let admitted = observer.quota_rejected_actual(&request, &rejected_lease, &terminal, values.factory());
        if matches!(scenario, "lease" | "closing") {
            assert!(admitted.is_err(), "{scenario}");
            owner.settle().await;
            assert!(account.list_credential_blocks(&[row_id]).unwrap().is_empty(), "{scenario}");
            continue;
        }
        assert_eq!(admitted.unwrap(), scenario != "unknown", "{scenario}");
        let canonical = canonical(values.clone(), &model, terminal.clone()).await;
        if scenario == "unknown" {
            assert!(owner.recorded_actual(&canonical, &values).unwrap().is_none());
            owner.settle().await;
            assert!(account.list_credential_blocks(&[row_id]).unwrap().is_empty());
            continue;
        }
        if scenario == "epoch" {
            host.admit(&model, Some(session.clone()), Some(session.clone()));
        }
        if scenario == "foreign" {
            let foreign = SessionValues::new(ValueHeap::new().unwrap(), Arc::new(NoRuntime)).unwrap();
            assert!(owner.recorded_actual(&canonical, &foreign).is_err());
        }
        if scenario == "runtime" {
            let other_runtime = SessionValues::new(values.heap().clone(), Arc::new(NoRuntime)).unwrap();
            assert!(owner.recorded_actual(&canonical, &other_runtime).unwrap().is_none());
        }
        if scenario == "public-forgery" {
            let forged = AgentMessage::from_graph(terminal.clone(), None).unwrap();
            assert!(owner.recorded_actual(&forged, &values).unwrap().is_none());
        }
        if scenario == "replacement" {
            canonical.replace_body(canonical.body_snapshot()).unwrap();
        }
        let observed = owner.recorded_actual(&canonical, &values).unwrap();
        if matches!(scenario, "epoch" | "replacement") {
            assert!(observed.is_none(), "{scenario}");
        } else {
            let observed = observed.expect("private canonical quota capability");
            assert!(owner.recorded_actual(&canonical, &values).unwrap().is_none(), "once-only consumed capability");
            let cancel = CancellationToken::new();
            if scenario == "known-cancel" {
                cancel.cancel();
            }
            let mut current_model = model.clone();
            if scenario == "model" {
                current_model.id = "new-model".into();
            }
            let logical = if scenario == "logical" { "other-logical-session" } else { &session };
            let outcome = recover(observed, &current_model, logical, false, &cancel).await.unwrap();
            assert_eq!(outcome.recorded, !matches!(scenario, "known-cancel" | "logical" | "model"), "{scenario}");
            assert!(!outcome.switched, "feedback does not grant replay/reset");
        }
        owner.settle().await;
        let blocks = account.list_credential_blocks(&[row_id]).unwrap();
        assert_eq!(blocks.len(), 1, "{scenario}: one durable original row rejection");
        let until = blocks[0].blocked_until_ms;
        // Replaying the same private request cannot make another feedback job,
        // nor extend its frozen clock. It has already left the observer map.
        assert!(observer.quota_rejected_actual(&request, &lease, &terminal, values.factory()).is_err());
        assert_eq!(account.list_credential_blocks(&[row_id]).unwrap()[0].blocked_until_ms, until);
        observer.finished(request.call_id);
    }
}
