//! Reference Host binding of fixed OMP 596f2da edit auto-repair/blackbox (MIT).
//! Borrows the admitted Session journal, registry, settings and AuthStorage;
//! each candidate is a side model request, never a new Agent/Session/Runtime.
use super::ProviderFactory;
use ara_ai::{
    AssistantBlock, AssistantMessage, CallOptions, Context, Message, Model, ModelProvider, StopReason, UserMessage,
};
use ara_tools::edit_post_apply::{EditPostApplyHost, EditRepairCompletion};
use async_trait::async_trait;
use futures::future::BoxFuture;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;
mod actual;
#[cfg(test)]
mod actual_tests;

pub(super) enum UsageObservation {
    Typed(AssistantMessage),
    Actual { record: ara_session::ModelUsageRecord, broker: Option<ara_cli::credential_store::ClientUsageEntry> },
}
impl UsageObservation {
    pub fn record(&self) -> ara_session::ModelUsageRecord {
        match self { Self::Typed(message) => usage_record(message), Self::Actual { record, .. } => record.clone() }
    }
    pub fn report(&self, account: &ara_cli::auth_storage::AuthStorage) {
        match self {
            Self::Typed(message) => account.record_assistant_usage(message),
            Self::Actual { broker: Some(row), .. } => account.record_observed_usage(std::slice::from_ref(row), None),
            Self::Actual { broker: None, .. } => {}
        }
    }
}
pub(super) type UsageCallback = Arc<dyn Fn(UsageObservation) -> BoxFuture<'static, Result<(), String>> + Send + Sync>;
pub(super) type UsageFactory = Arc<dyn Fn() -> BoxFuture<'static, Result<UsageCallback, String>> + Send + Sync>;
pub(super) type ActiveModelCallback = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// OMP reads agent.state.model when the post-apply recorder is created, not
/// when a Run enters. Borrow the existing current Host snapshot without a
/// context -> facade -> hooks -> tools -> context ownership cycle.
pub(super) fn active_model_for_hooks(model: &Model, hooks: &Arc<dyn ara_agent::LoopHooks>) -> ActiveModelCallback {
    let fallback = format!("{}/{}", model.provider, model.id);
    let owner = Arc::downgrade(hooks);
    Arc::new(move || {
        let hooks = owner.upgrade()?;
        let current = hooks.execution_snapshot().and_then(|snapshot| snapshot.model);
        Some(
            current
                .map(|current| format!("{}/{}", current.model.provider, current.model.id))
                .unwrap_or_else(|| fallback.clone()),
        )
    })
}

pub(super) struct HostEditPostApply {
    factory: ProviderFactory,
    project: PathBuf,
    config_path: Option<PathBuf>,
    agent_dir: PathBuf,
    session_id: String,
    active_model: ActiveModelCallback,
    usage: UsageFactory,
}
impl HostEditPostApply {
    pub(super) fn install(
        context: &ara_tools::ToolContext,
        factory: &ProviderFactory,
        project: PathBuf,
        config_path: Option<PathBuf>,
        session_id: String,
        active_model: ActiveModelCallback,
        usage: UsageFactory,
    ) {
        context.set_edit_post_apply_host(Arc::new(Self {
            factory: factory.borrowed_side_factory(),
            project,
            config_path,
            agent_dir: super::ara_home().join("agent"),
            session_id,
            active_model,
            usage,
        }));
    }
}
#[async_trait]
impl EditPostApplyHost for HostEditPostApply {
    fn agent_dir(&self) -> PathBuf {
        self.agent_dir.clone()
    }
    fn active_model_string(&self) -> Option<String> {
        (self.active_model)()
    }
    fn blackbox_enabled(&self) -> bool {
        match super::rpc_host_settings::EditPostApplySettings::load(&self.agent_dir) {
            Ok(settings) => settings.blackbox_enabled,
            Err(error) => {
                self.diagnostic(&format!("Edit blackbox settings unavailable: {error}"));
                false
            }
        }
    }
    fn diagnostic(&self, message: &str) {
        eprintln!("ara: {message}")
    }
    async fn prepare_repair(
        &self,
        cancel: Option<CancellationToken>,
    ) -> Result<Option<Arc<dyn EditRepairCompletion>>, String> {
        let settings =
            super::rpc_host_settings::EditPostApplySettings::load(&self.agent_dir).map_err(|e| e.to_string())?;
        if !settings.auto_repair_enabled {
            return Ok(None);
        }
        let Some(registry) = self.factory.registry.clone() else { return Ok(None) };
        let lookup = |role: &str| ara_cli::model_roles::model_role_value(settings.roles.get(role));
        let order = ara_cli::model_usage_order::SqliteModelUsageOrder::for_agent_dir(self.agent_dir.clone());
        let resolver = registry
            .model_role_resolver(ara_cli::model_roles::ModelMatchPreferences {
                provider_order: settings.provider_order.clone(),
                usage_order: order.get_model_usage_order().unwrap_or_else(|e| {
                    self.diagnostic(&format!("Edit repair model preference order unavailable: {e}"));
                    Vec::new()
                }),
                ..Default::default()
            })
            .map_err(|e| e.to_string())?;
        let Some(choice) = resolver.resolve_role_selection(&["smol"], &lookup).map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let spec: serde_json::Value =
            serde_json::from_str(&choice.model.spec().to_wire_json().stringify()).map_err(|e| e.to_string())?;
        let Some(provider) = spec["provider"].as_str() else { return Ok(None) };
        let Some(id) = spec["id"].as_str() else { return Ok(None) };
        let selector = ara_cli::retry_fallback::RetryFallbackSelector {
            raw: format!("{provider}/{id}"),
            provider: provider.into(),
            id: id.into(),
            thinking_level: None,
        };
        let path = self.config_path.clone().unwrap_or_else(|| self.agent_dir.join("models.yml"));
        let models = ara_cli::daily_model_config::load_daily_config(&path, self.config_path.is_some())
            .map_err(|e| e.to_string())?;
        let mut selection = ara_cli::daily_model_config::resolve_daily_role_selection(
            models.as_ref(),
            &selector,
            &ara_cli::schema_policy::process_daily_environment,
            &choice.model,
        )
        .map_err(|e| e.to_string())?;
        // Fixed Codex provider drops completeSimple's maxTokens on its wire.
        if let ara_cli::model_route::ProtocolOptions::CodexResponses(options) = &mut selection.protocol {
            options.request.max_tokens = None;
        }
        let cancel = cancel.unwrap_or_default();
        let side = self
            .factory
            .prepare_daily_replacement(
                selection,
                Some(self.session_id.clone()),
                Some(self.session_id.clone()),
                &self.project,
                &cancel,
            )
            .await
            .map_err(|e| e.to_string())?;
        let keyless = registry.is_keyless_provider(&selector.provider.as_str().into());
        if side.route.check_oneshot_api_key(&cancel, keyless).await.is_err() {
            return Ok(None);
        }
        let usage = (self.usage)().await?;
        Ok(Some(Arc::new(Completion {
            provider: side.route.bind_side_request(side.client.clone(), &self.session_id),
            model: side.route.model().clone(),
            usage,
        })))
    }
}

struct Completion {
    provider: Arc<dyn ModelProvider>,
    model: Model,
    usage: UsageCallback,
}
#[async_trait]
impl EditRepairCompletion for Completion {
    fn model_name(&self) -> String {
        format!("{}/{}", self.model.provider, self.model.id)
    }
    fn complete_actual(&self, invocation: ara_tools::edit_post_apply::EditNativeRepairInvocation, prompt: String, cancel: CancellationToken) -> BoxFuture<'static, Result<String, ara_rpc::value_heap::ValueError>> {
        actual::complete(self.provider.clone(), self.model.clone(), self.usage.clone(), invocation, prompt, cancel)
    }
    async fn complete(&self, prompt: String, cancel: CancellationToken) -> Result<String, String> {
        // Source completeSimple's inner loop retries + transient oneshot outer
        // retries share this side provider and its private auth resolver.
        for attempt in 1..=3 {
            let context = Context {
                system_prompt: Vec::new(),
                messages: vec![Message::User(UserMessage::text(prompt.clone()))],
                tools: None,
            };
            let options = CallOptions {
                cancel: cancel.clone(),
                max_tokens: if self.model.api == "openai-codex-responses" { None } else { Some(8192) },
                reasoning: Some(ara_ai::reasoning::ReasoningRequest { disable_reasoning: true, ..Default::default() }),
                ..Default::default()
            };
            let mut observed = Vec::new();
            let result = ara_ai::thinking_loop::complete_with_thinking_loop_retry_observed(
                self.provider.as_ref(),
                &self.model,
                &context,
                options,
                |message| observed.push(message.clone()),
            )
            .await;
            for message in observed {
                (self.usage)(UsageObservation::Typed(message)).await?;
            }
            let message = match result {
                Ok(message) => message,
                Err(ara_ai::ProviderError::Incomplete(_)) if attempt < 3 => {
                    wait(
                        &cancel,
                        Duration::from_secs_f64(
                            ara_cli::auto_thinking::backoff_delay_ms(
                                attempt,
                                ara_cli::auto_thinking::jitter_fraction(),
                            ) / 1000.0,
                        ),
                    )
                    .await?;
                    continue;
                }
                Err(ara_ai::ProviderError::Aborted) => return Err("auto-repair aborted".into()),
                Err(error) => return Err(error.to_string()),
            };
            if message.stop_reason == StopReason::Aborted {
                return Err("auto-repair aborted".into());
            }
            if message.stop_reason != StopReason::Error {
                return Ok(message
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        AssistantBlock::Text(t) => Some(t.text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(""));
            }
            if cancel.is_cancelled() {
                return Err(message.error_message.unwrap_or_else(|| "auto-repair completion cancelled".into()));
            }
            let class = ara_ai::retry_classification::classify_retry(&message, &self.model.api);
            // Fixed caller passes no getResponseHeaders, so outer oneshot hints
            // come from error text; never inherit provider-internal header caps.
            let mut text_message = message.clone();
            text_message.failure_evidence = None;
            let delay = repair_retry_delay(&text_message, &self.model.provider, attempt, ara_cli::auto_thinking::jitter_fraction());
            if attempt == 3 || !ara_cli::auto_thinking::retryable_oneshot(&message, &class) || delay.is_none() {
                return Err(message.error_message.unwrap_or_else(|| "auto-repair completion failed".into()));
            }
            wait(&cancel, delay.expect("checked oneshot delay")).await?;
        }
        unreachable!("third attempt returns")
    }
}
/// Typed adapter to target retryTransientCompletion({signal, provider}). The
/// provider hint supplies offsetless reset interpretation; API-only UTC hints
/// must not override a provider's wall-clock timezone.
fn repair_retry_delay(message: &AssistantMessage, provider: &str, attempt: u32, jitter: f64) -> Option<Duration> {
    let hint = ara_ai::retry_hint::extract_provider_retry_hint(
        Some(provider), message.error_message.as_deref(), chrono::Utc::now().timestamp_millis() as f64,
    );
    if hint.is_some_and(|hint| hint > 30_000.0) { return None; }
    let backoff = ara_cli::auto_thinking::backoff_delay_ms(attempt, jitter);
    Some(Duration::from_secs_f64(hint.unwrap_or(0.0).max(backoff).min(30_000.0) / 1000.0))
}

async fn wait(cancel: &CancellationToken, duration: Duration) -> Result<(), String> {
    tokio::select! {biased;_=cancel.cancelled()=>Err("auto-repair cancelled".into()),_=tokio::time::sleep(duration)=>Ok(())}
}

pub(super) fn usage_record(message: &AssistantMessage) -> ara_session::ModelUsageRecord {
    ara_session::ModelUsageRecord {
        purpose: "edit-auto-repair".into(),
        role: Some("smol".into()),
        api: message.api.clone(),
        provider: message.provider.clone(),
        model: message.model.clone(),
        usage: message.usage.clone(),
        stop_reason: message.stop_reason,
        error_message: message.error_message.clone(),
    }
}

/// Installs at each CLI/REPL Run boundary. The side call captures the original
/// current journal owner before issuing any candidate and retains its sink.
pub(super) async fn install_cli(
    args: &super::Args,
    factory: &ProviderFactory,
    project: &std::path::Path,
    owner: &super::CliToolOwner,
    model: &Model,
    hooks: &Arc<dyn ara_agent::LoopHooks>,
    sink: &Arc<super::HostSink>,
) {
    let Some(context) = &owner.context else { return };
    let captured = Arc::downgrade(sink);
    let account = factory.shared_account.clone();
    let usage: UsageFactory = Arc::new(move || {
        let captured = captured.clone();
        let account = account.clone();
        Box::pin(async move {
            let captured = captured.upgrade().ok_or("Edit repair original Session closed")?;
            let owner =
                captured.journal.lock().await.as_ref().ok_or("Edit repair Session unavailable")?.model_usage_owner();
            let owner = Arc::new(tokio::sync::Mutex::new(owner));
            let usage: UsageCallback = Arc::new(move |message| {
                let captured = captured.clone();
                let account = account.clone();
                let owner = owner.clone();
                Box::pin(async move {
                    let record = message.record();
                    let mut owner = owner.lock().await;
                    let mut journal = captured.journal.lock().await;
                    let journal = journal.as_mut().ok_or("Edit repair Session unavailable")?;
                    match journal.append_model_usage(&record, &owner) {
                        Ok(Some(id)) => owner.parent_id = Some(id),
                        Ok(None) => {}
                        Err(error) => {
                            captured.persistence_failure(&error);
                            return Err("Edit repair usage persistence failed".into());
                        }
                    }
                    if let Some(account) = account {
                        message.report(&account);
                    }
                    Ok(())
                })
            });
            Ok(usage)
        })
    });
    // This facade is installed after journal allocation, whose id is also
    // present on the existing provider factory. REPL /new rebinds the sink;
    // resolve the actual id below rather than inheriting factory startup id.
    let Some(session) = sink.journal.lock().await.as_ref().map(|journal| journal.session_id().to_owned()) else {
        return;
    };
    HostEditPostApply::install(
        context,
        factory,
        project.to_path_buf(),
        args.models_config.clone(),
        session,
        active_model_for_hooks(model, hooks),
        usage,
    );
}


#[cfg(test)]
mod upgrade_tests {
    use super::*;
    use ara_ai::{AssistantMessageEvent,EventSink};
    use std::sync::atomic::{AtomicUsize,Ordering};
    struct AbortedProvider(AtomicUsize);
    impl ModelProvider for AbortedProvider {
        fn stream(&self,_:&Model,_:&Context,_:CallOptions)->ara_ai::AssistantStream {
            self.0.fetch_add(1,Ordering::SeqCst);
            let (sink,stream)=EventSink::channel();
            tokio::spawn(async move {
                let mut message=AssistantMessage::empty("openai-completions","fixture","repair");
                message.stop_reason=StopReason::Aborted;
                sink.push(AssistantMessageEvent::Error {reason:StopReason::Aborted,error:message}).await;
            });stream
        }
    }
    #[tokio::test]
    async fn typed_aborted_terminal_is_not_an_empty_repair_candidate() {
        let provider=Arc::new(AbortedProvider(AtomicUsize::new(0)));
        let usages=Arc::new(AtomicUsize::new(0));
        let usage:UsageCallback={let usages=usages.clone();Arc::new(move|_|{usages.fetch_add(1,Ordering::SeqCst);Box::pin(async{Ok(())})})};
        let completion=Completion {provider:provider.clone(),usage,model:Model {id:"repair".into(),api:"openai-completions".into(),provider:"fixture".into(),base_url:"http://unused.invalid".into(),reasoning:false,max_tokens:None,context_window:None,tokenizer:None}};
        assert_eq!(completion.complete("repair".into(),CancellationToken::new()).await.unwrap_err(),"auto-repair aborted");
        assert_eq!(provider.0.load(Ordering::SeqCst),1);assert_eq!(usages.load(Ordering::SeqCst),1);
    }
    #[test]
    fn typed_repair_retry_uses_current_model_provider_timezone() {
        let mut message=AssistantMessage::empty("openai-completions","zai","repair");
        message.stop_reason=StopReason::Error;
        let future=chrono::Utc::now()+chrono::Duration::hours(8)+chrono::Duration::seconds(3);
        message.error_message=Some(format!("429 rate limit will reset at {}",future.format("%Y-%m-%d %H:%M:%S")));
        let delay=repair_retry_delay(&message,"zai",1,0.0).unwrap();
        assert!(delay<std::time::Duration::from_secs(5));
        assert!(repair_retry_delay(&message,"other-provider",1,0.0).is_none());
    }
}
