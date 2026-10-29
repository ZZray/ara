//! First production RPC host slice. Fixed OMP `rpc-mode.ts` at
//! 596f2da7101178214aa27a753529d15e6b7ad91d; complete command parity remains open.
//!
//! Input, serial commands, owned Runs and ordered stdout have distinct owners.
//! A Run keeps its original journal even if the caller stops waiting. Live
//! queries use completed event messages, never the Agent transcript lock.

use super::rpc_host_settings::{
    AutoCompactionPolicy, LoopGuardSettings, RetryPolicy, ToolLoopGuardSettings, UnexpectedStopMode,
};
#[path = "rpc_host_auto.rs"]
mod auto;
#[cfg(test)]
#[path = "rpc_host_core_hooks_tests.rs"]
mod core_hooks_tests;
#[path = "rpc_host_fallback.rs"]
mod fallback;
#[path = "rpc_host_handoff.rs"]
mod handoff;
#[cfg(test)]
#[path = "rpc_host_local_reduction_tests.rs"]
mod local_reduction_tests;
#[path = "rpc_host_loop_guard.rs"]
mod loop_guard;
#[path = "rpc_host_maintenance.rs"]
mod maintenance;
#[path = "rpc_host_models.rs"]
mod models;
#[path = "rpc_host_native_loop.rs"]
mod native_loop;
#[cfg(test)]
#[path = "rpc_host_native_loop_tests.rs"]
mod native_loop_tests;
#[path = "rpc_host_quota.rs"]
mod quota;
#[path = "rpc_host_reduction.rs"]
mod reduction;
#[path = "rpc_host_remote.rs"]
mod remote;
#[path = "rpc_host_snapcompact.rs"]
mod snapcompact;
#[path = "rpc_host_terminal.rs"]
mod terminal;
use super::rpc_host_retry::NativeRetryContextReceipt;
use super::rpc_host_tools::{HostToolDefinition, ToolBridge, normalize_host_tool_definitions};
use super::rpc_host_uris::UriBridge;
use anyhow::{Context as _, Result, bail};
use ara_agent::{
    ActualToolSource, Agent, AgentConfig, AgentError, AgentEvent, AgentEventSink, AgentInput,
    AgentMessage, ExecutionSnapshot, LoopHooks, QueueMode, RawContextError, RunEnd, RunReport,
    SessionValues, ToolDecision,
};
use ara_ai::{
    AssistantMessage, AssistantRetryRecovery, ImageContent, Message, UserBlock, UserContent,
    UserMessage,
};
use ara_discovery::LoadedSkill;
use ara_rpc::{
    WireValue,
    frame::*,
    input::{InputItem, RpcInputReader},
    writer::RpcOutput,
};
use ara_session::{
    BashExecutionMessage, LoopGuardNotice, SessionJournal, UserSkillPrompt,
    bash_output_meta_from_summary,
};
use async_trait::async_trait;
use loop_guard::{GeminiHeaderGuard, HeaderInterruption, HeaderTarget, ToolLoopState};
use serde_json::{Value, json};
use std::{
    future::{Future, pending},
    path::PathBuf,
    pin::Pin,
    sync::{Arc, Mutex, RwLock, Weak},
    time::{Duration, Instant},
};
use terminal::{NativeRetryWaitGuard, NativeTerminalImports, RecoveryTerminal};
#[cfg(test)]
pub(super) use terminal::observe_native_retry_wait_guard_family;

fn lexical_absolute(path: &std::path::Path) -> Result<PathBuf> {
    // Fixed path.resolve comparison does not inspect symlinks or require the
    // directory to exist. std::path::absolute supplies platform drive/root
    // semantics; fold remaining parent components without filesystem I/O.
    let absolute = std::path::absolute(path)?;
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::CurDir => {}
            component => normalized.push(component.as_os_str()),
        }
    }
    Ok(normalized)
}

/// A factory result keeps the entire Native preparation alive. Bare values
/// are admitted only by the existing controlled fixture build.
pub(super) enum PreparedActualSession {
    Native(Arc<ara_cli::native_factory::PreparedNativeSession>),
    #[cfg(test)]
    FixtureValues(Arc<SessionValues>),
}
impl PreparedActualSession {
    fn values(&self) -> &Arc<SessionValues> {
        match self {
            Self::Native(owner) => owner.values(),
            #[cfg(test)]
            Self::FixtureValues(values) => values,
        }
    }
    fn native_session(&self) -> Option<Arc<ara_cli::native_factory::PreparedNativeSession>> {
        match self {
            Self::Native(owner) => Some(owner.clone()),
            #[cfg(test)]
            Self::FixtureValues(_) => None,
        }
    }
}
pub(super) type ActualSessionValuesFactory =
    Arc<dyn Fn() -> Result<PreparedActualSession> + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct HostAgentIdentity {
    pub definition_name: Option<String>,
    pub effective_name: String,
}
pub(super) struct HostAgentContext {
    pub identity: HostAgentIdentity,
    pub auth_scope: Arc<ara_cli::auth_storage::AuthSessionScope>,
}
impl HostAgentContext {
    pub fn for_primary(self: &Arc<Self>, id: &str) -> Arc<Self> {
        if self.auth_scope.primary_session_id == id { return self.clone(); }
        Arc::new(Self { identity: self.identity.clone(), auth_scope: self.auth_scope.for_primary(id) })
    }
    #[cfg(feature="test-fixture")]
    fn receipt(&self) -> Value {
        json!({"definitionName":self.identity.definition_name,"effectiveName":self.identity.effective_name,
            "primarySessionId":self.auth_scope.primary_session_id,"pools":self.auth_scope.oauth_account_pools})
    }
}

pub(super) struct SessionFactory {
    pub peer_messaging_enabled: bool,
    pub agent_context: Option<Arc<HostAgentContext>>,
    pub peers: Option<Arc<super::peer_host::BinaryPeerHost>>,
    pub peer_root: Option<String>,
    pub peer_attachment:Option<ara_cli::host_peers::AttachmentExpectation>,
    pub dir: Option<PathBuf>,
    pub cwd: PathBuf,
    pub provider: super::ProviderFactory,
    pub args: super::Args,
    pub explicit_generation: ara_cli::daily_model_config::DailyGeneration,
    pub mcp_config: Option<ara_mcp::ServerConfig>,
    pub tool_owner: super::CliToolOwner,
    pub uri_port: Option<Arc<dyn ara_tools::ContentUriPort>>,
    pub startup: Option<super::startup_selection::StartupState>,
    pub context_controller: Option<ara_cli::append_only_context_mode::SessionContextController>,
    pub actual_values_factory: Option<ActualSessionValuesFactory>,
    /// Main supplies its already prepared initial owner, prepared together
    /// with the exact initial hooks/tools before entering serve.
    pub initial_native_session: Option<Arc<ara_cli::native_factory::PreparedNativeSession>>,
    pub native_jobs: super::native_setup::NativeSessionJobs,
    pub acp_plan: Option<Arc<super::acp_plan::AcpPlan>>,
}

struct PreparedSessionConfig {
    values: Option<Arc<SessionValues>>,
    native_session: Option<Arc<ara_cli::native_factory::PreparedNativeSession>>,
    native_publication: Option<super::native_setup::NativeSetupPublication>,
    config: AgentConfig,
    skills: Vec<LoadedSkill>,
    tool_owner: super::CliToolOwner,
    context_controller: Option<ara_cli::append_only_context_mode::SessionContextController>,
}

impl SessionFactory {
    fn messaging_eligibility(&self,depth:usize)->ara_cli::host_peers::agent_uri::MessagingEligibility{
        let cwd=self.cwd.clone();let agent_dir=super::ara_home().join("agent");let enabled=self.peer_messaging_enabled;
        Arc::new(move||super::rpc_host_settings::peer_messaging_enabled(&cwd,&agent_dir,depth,enabled).map_err(|error|error.to_string()))
    }

    async fn config(
        &self,
        current: &AgentConfig,
        metadata: Option<&Value>,
        retained_skills: Option<&[LoadedSkill]>,
        reset: bool,
        values: Option<Arc<SessionValues>>,
        native_session: Option<Arc<ara_cli::native_factory::PreparedNativeSession>>,
        mut overlay: super::ToolOverlay,
        retained_owner: Option<super::CliToolOwner>,
    ) -> Result<PreparedSessionConfig> {
        validate_native_session_values(&values, &native_session)?;
        if !reset {
            validate_session_values(current, self.context_controller.as_ref(), &values)?;
        }
        let captured_rpc = overlay.native_owners.clone();
        // The configured WRITE remains selected on exit. Only the missing
        // WRITE supplied for plan transport is added/removed by this owner.
        let plan_write_selection = self
            .acp_plan
            .as_ref()
            .filter(|_| {
                self.args
                    .tools
                    .as_deref()
                    .is_some_and(|tools| !tools.split(',').any(|name| name.trim() == "write"))
            })
            .map(|plan| plan.snapshot_mode() == "plan");
        let mut effective_args = self.args.clone();
        if plan_write_selection == Some(true) {
            let tools = effective_args.tools.get_or_insert_with(String::new);
            if !tools.is_empty() {
                tools.push(',');
            }
            tools.push_str("write");
        }
        let native_retained = retained_owner
            .as_ref()
            .and_then(|owner| owner.native.as_ref())
            .zip(native_session.as_ref())
            .is_some_and(|(owner, selected)| Arc::ptr_eq(&owner.prepared, selected));
        let retained_owner = retained_owner.unwrap_or_else(|| self.tool_owner.clone());
        let retained_child=retained_owner.native.as_ref().and_then(|native|native.child_contract.clone());
        if retained_child.is_some(){
            overlay.child_read_summarize=retained_owner.context.as_ref().map(|context|context.read_summary_policy().enabled);
            overlay.child_restrict_lsp=retained_owner.lsp_startup.as_ref().is_some_and(|lsp|!lsp.snapshot.settings.enabled);
        }
        overlay.read_host = retained_owner.read_host.clone();
        overlay.read_video = retained_owner.read_video.clone();
        let uri_base = retained_owner.uri_base.clone().or_else(|| {
            retained_owner
                .uri_artifacts
                .is_none()
                .then(|| self.uri_port.clone())
                .flatten()
        });
        let uri_artifacts = retained_owner.uri_artifacts.clone();
        let uri_peer=retained_owner.uri_peer.clone();
        let retained_tools = if native_retained {
            let prepared = native_session
                .as_ref()
                .expect("native_retained requires selected Session");
            let source = prepared.require_catalog()?;
            let array = source
                .current_tools(prepared.values())?
                .context("Retained Native Session has no catalog")?;
            let mut tools = Vec::new();
            for index in 0..prepared
                .values()
                .heap()
                .array_length(&array, prepared.values().runtime())?
            {
                let selected = prepared.values().heap().get(
                    &array,
                    &index.to_string().as_str().into(),
                    prepared.values().runtime(),
                )?;
                let capability = source
                    .bind_selected(&selected, prepared.values())?
                    .capability;
                let is_rpc = retained_owner.native.as_ref().is_some_and(|owner| {
                    owner
                        .rpc
                        .iter()
                        .any(|member| Arc::ptr_eq(&member.binding.capability, &capability))
                });
                if !is_rpc {
                    tools.push(capability);
                }
            }
            Some(super::RetainedToolSetup {
                tools,
                owner: retained_owner,
                reset,
                fresh_edit_session: false,
                plan_write_selection,
                context_controller: (!reset).then(|| self.context_controller.clone()).flatten(),
                projector: None,
                projection_journal: None,
            })
        } else {
            (!reset).then(|| super::RetainedToolSetup {
                tools: current.tools.clone(),
                owner: retained_owner,
                reset: false,
                fresh_edit_session: native_session.is_some(),
                plan_write_selection,
                context_controller: self.context_controller.clone(),
                projector: None,
                projection_journal: None,
            })
        };
        let mut setup = super::boxed_cli_setup_with_retained_skills(
            &effective_args,
            &self.cwd,
            &current.model,
            metadata,
            &self.mcp_config,
            // Preserve the established fresh RPC Session preparation,
            // including MCP reconnect and its failure-before-adoption boundary.
            retained_tools,
            overlay,
            retained_skills.map(<[LoadedSkill]>::to_vec),
        )
        .await?;
        if let Some(child)=&retained_child {setup.system_prompt=child.published_snapshot().system_prompt;}
        // A fresh reset rebuilds the stable routing skeleton as well. The
        // served self.uri_port includes the old vault snapshot and must not
        // become the next snapshot's base. Artifact binding remains shared.
        if setup.tool_owner.uri_artifacts.is_none() {
            setup.tool_owner.uri_base = uri_base;
            setup.tool_owner.uri_peer=uri_peer;
            setup.tool_owner.uri_artifacts = uri_artifacts;
            if let Some(context) = &setup.tool_owner.context {
                if let Some(port) = super::prepared_host_uri(
                    setup.tool_owner.vault.as_ref(),
                    setup.tool_owner.uri_base.as_ref(),
                    setup.tool_owner.uri_peer.as_ref(),
                    setup.tool_owner.uri_artifacts.as_ref(),
                ) {
                    context.set_uri_port(port);
                }
            }
        }
        let native_publication = if let Some(draft) = &native_session {
            let mut options = super::native_setup_options(&setup, draft, &captured_rpc)?;
            options.proposal_host = self.acp_plan.as_ref().map(|plan| {
                plan.clone() as Arc<dyn ara_cli::native_factory::catalog::NativeProposalHost>
            });
            let publication = super::native_setup::prepare_cli_native(
                &mut setup,
                draft.clone(),
                captured_rpc,
                &self.cwd,
                &current.model,
                options,
            )?;
            super::native_setup::remember_session_jobs(
                &self.native_jobs,
                &publication.native_owner,
            );
            Some(publication)
        } else {
            None
        };
        let native_session = native_publication
            .as_ref()
            .map(|publication| publication.prepared.clone())
            .or(native_session);
        // Do not overwrite the prepared slot with the old artifact fallback:
        // it may contain a vault gate from the previous settings snapshot.
        let context_controller = if reset {
            Some(setup.context_controller)
        } else {
            self.context_controller.clone()
        };
        if let Some(controller) = &context_controller {
            controller.stage_actual_values(values.clone())?;
        }
        let mut config = current.clone();
        config.system_prompt = setup.system_prompt;
        config.tools = setup.tools;
        if reset {
            config.provider = self.provider.build();
        }
        if reset || native_publication.is_some() {
            config.hooks = setup.hooks;
        }
        validate_session_values(&config, context_controller.as_ref(), &values)?;
        validate_native_session_hooks(&config, &values, &native_session)?;
        Ok(PreparedSessionConfig {
            values,
            native_session,
            native_publication,
            config,
            skills: setup.skills,
            tool_owner: setup.tool_owner,
            context_controller,
        })
    }
}

use ara_cli::agent_message_projection::{raw_last_assistant, raw_retry_tail};

struct RpcHooks {
    base: Arc<dyn LoopHooks>,
    snapshot: Arc<RwLock<ExecutionSnapshot>>,
    forced_child_yield: bool,
    admission: Option<fallback::AdmissionBinding>,
    publication: Arc<ara_cli::append_only_context_mode::SnapshotPublication>,
    projector: Arc<ara_cli::agent_message_projection::AgentMessageProjector>,
    session: Arc<Session>,
}

#[async_trait]
impl LoopHooks for RpcHooks {
    fn actual_tool_source(
        &self,
        values: &SessionValues,
    ) -> Result<Option<Arc<dyn ara_agent::ActualToolSource>>, ara_agent::ActualProviderError> {
        self.base.actual_tool_source(values)
    }
    fn supports_actual_tool_hooks(&self) -> bool {
        self.base.supports_actual_tool_hooks()
    }
    async fn before_tool_dispatch_actual(
        &self,
        context: ara_agent::ActualBeforeToolCallContext<'_>,
        cancel: &CancellationToken,
    ) -> Result<Option<ara_rpc::value_heap::ValueHandle>, ara_agent::ActualProviderError> {
        self.base.before_tool_dispatch_actual(context, cancel).await
    }
    async fn before_tool_call_actual(
        &self,
        context: ara_agent::ActualBeforeToolCallContext<'_>,
        cancel: &CancellationToken,
    ) -> Result<ara_agent::ActualToolDecision, ara_agent::ActualProviderError> {
        self.base.before_tool_call_actual(context, cancel).await
    }
    async fn after_tool_call_actual(
        &self,
        context: ara_agent::ActualAfterToolCallContext<'_>,
        cancel: &CancellationToken,
    ) -> Result<Option<ara_rpc::value_heap::ValueHandle>, ara_agent::ActualProviderError> {
        self.base.after_tool_call_actual(context, cancel).await
    }
    fn transform_tool_call_arguments_actual(
        &self,
        args: ara_rpc::value_heap::ValueHandle,
        name: &ara_rpc::WireString,
        values: &SessionValues,
    ) -> Result<ara_rpc::value_heap::ValueHandle, ara_agent::ActualProviderError> {
        self.base
            .transform_tool_call_arguments_actual(args, name, values)
    }
    fn get_tool_context_actual(
        &self,
        context: &ara_agent::ActualToolExecutionContext,
        values: &SessionValues,
    ) -> Result<ara_rpc::value_heap::ValueHandle, ara_agent::ActualProviderError> {
        self.base.get_tool_context_actual(context, values)
    }
    fn tool_error_message_actual(
        &self,
        error: &ara_rpc::value_heap::ValueError,
        values: &SessionValues,
    ) -> Result<ara_rpc::value_heap::ValueHandle, ara_agent::ActualProviderError> {
        self.base.tool_error_message_actual(error, values)
    }
    fn sanitize_tool_result_text_actual(
        &self,
        text: &ara_rpc::value_heap::ValueHandle,
        values: &SessionValues,
    ) -> Result<ara_rpc::value_heap::ValueHandle, ara_agent::ActualProviderError> {
        self.base.sanitize_tool_result_text_actual(text, values)
    }
    fn soft_tool_satisfies_actual(
        &self,
        requirement: &ara_agent::soft_tool_requirement::SoftToolRequirement,
        call: &ara_rpc::value_heap::ValueHandle,
        values: &SessionValues,
    ) -> Result<bool, ara_agent::ActualProviderError> {
        self.base
            .soft_tool_satisfies_actual(requirement, call, values)
    }
    fn final_provider_gate_binding(&self) -> Option<ara_agent::FinalProviderGateBinding> {
        self.base.final_provider_gate_binding()
    }
    fn on_final_provider_gate_rejected(
        &self,
    ) -> std::result::Result<(), ara_agent::FinalProviderGateError> {
        self.base.on_final_provider_gate_rejected()
    }
    fn select_tool_choice_directive(
        &self,
        active_tools: &[Arc<dyn ara_agent::AgentTool>],
        admitted_static_choice: Option<&ara_ai::ToolChoice>,
    ) -> std::result::Result<ara_agent::ToolChoiceSelection, ara_agent::ToolDirectiveError> {
        self.base
            .select_tool_choice_directive(active_tools, admitted_static_choice)
    }
    fn select_tool_choice_directive_full(
        &self,
        active_tools: &[Arc<dyn ara_agent::AgentTool>],
        admitted_static_choice: Option<&ara_ai::ToolChoice>,
    ) -> std::result::Result<
        ara_agent::soft_tool_requirement::ToolChoiceDirectiveSelection,
        ara_agent::ToolDirectiveError,
    > {
        self.base
            .select_tool_choice_directive_full(active_tools, admitted_static_choice)
    }
    fn soft_tool_requirement_state(
        &self,
    ) -> Option<ara_agent::soft_tool_requirement::SoftToolRequirementState> {
        self.base.soft_tool_requirement_state()
    }

    fn on_deferred_tool_choice_unavailable(
        &self,
    ) -> std::result::Result<(), ara_agent::ToolDirectiveError> {
        self.base.on_deferred_tool_choice_unavailable()
    }
    fn provider_context_binding(&self) -> Option<ara_agent::ProviderContextBinding> {
        self.snapshot
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .provider_context
            .clone()
    }
    fn set_append_only_context(
        &self,
        manager: Option<ara_agent::SharedAppendOnlyContextManager>,
    ) -> std::result::Result<(), ara_agent::ProviderContextError> {
        self.publication.set_append_only_context(manager)
    }

    fn queued_input_pending(&self, kind: ara_agent::QueueInputKind) -> Option<bool> {
        self.base.queued_input_pending(kind)
    }
    fn context_admission_enabled(&self) -> bool {
        self.admission.is_some() || self.base.context_admission_enabled()
    }
    async fn before_queued_input_dequeue_with_raw_context(
        &self,
        snapshot: ExecutionSnapshot,
        messages: Vec<AgentMessage>,
        cancel: &CancellationToken,
    ) -> Result<(), ara_agent::AdmissionError> {
        self.base
            .before_queued_input_dequeue_with_raw_context(
                snapshot.clone(),
                messages.clone(),
                cancel,
            )
            .await?;
        if let Some(binding) = &self.admission {
            binding.request(false, snapshot, messages, cancel).await?;
        }
        Ok(())
    }
    async fn before_model_call_with_raw_context(
        &self,
        snapshot: ExecutionSnapshot,
        messages: Vec<AgentMessage>,
        cancel: &CancellationToken,
    ) -> Result<ExecutionSnapshot, ara_agent::AdmissionError> {
        let snapshot = self
            .base
            .before_model_call_with_raw_context(snapshot, messages.clone(), cancel)
            .await?;
        let child=self.session.tool_owner.lock().unwrap().native.as_ref().is_some_and(|n|n.child_contract.is_some());
        if child {
            let mut journal=self.session.journal.lock().await;
            let previous=journal.latest_child_init().map_err(|e|ara_agent::AdmissionError::Blocked(e.to_string()))?.ok_or_else(||ara_agent::AdmissionError::Blocked("Child init missing before model admission".into()))?;
            let mut init=previous.clone();init.system_prompt=snapshot.system_prompt.clone();
            init.tools=snapshot.tools.iter().map(|tool|tool.definition().name).filter(|name|init.read_only!=Some(true)||name!="write").collect();
            if let Some(model)=&snapshot.model {init.resolved_model=Some(format!("{}/{}",model.model.provider,model.model.id));}
            if init!=previous {journal.append_child_init(&init).map_err(|e|ara_agent::AdmissionError::Blocked(format!("Child model contract capture failed: {e}")))?;}
        }
        match &self.admission {
            Some(binding) => binding.request(true, snapshot, messages, cancel).await,
            None => Ok(snapshot),
        }
    }
    async fn transform_agent_context(
        &self,
        live: &mut Vec<AgentMessage>,
        cancel: &CancellationToken,
    ) -> Result<Option<Vec<AgentMessage>>, RawContextError> {
        self.base.transform_agent_context(live, cancel).await
    }
    async fn convert_agent_messages_to_llm(
        &self,
        messages: &[AgentMessage],
    ) -> Result<Vec<Message>, RawContextError> {
        let snapshot = self
            .snapshot
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let model = snapshot.model.as_ref().ok_or_else(|| {
            RawContextError("RPC raw projection requires an admitted model route".into())
        })?;
        let journal = self.session.journal.lock().await;
        self.projector
            .refresh_checked(&journal, messages, &model.model)?;
        drop(journal);
        self.projector.convert_current(messages, &model.model)
    }
    async fn convert_agent_messages_to_llm_actual(
        &self,
        messages: &[AgentMessage],
        values: &SessionValues,
    ) -> Result<Vec<ara_rpc::value_heap::ValueHandle>, ara_agent::ActualProviderError> {
        let snapshot = self
            .snapshot
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let model = snapshot
            .model
            .as_ref()
            .ok_or(ara_agent::ActualProviderError::Protocol(
                "RPC actual projection requires an admitted model route",
            ))?;
        // This Session owns the projector. Native summary consumers remain
        // explicit; never use the startup projector or legacy refresh/export.
        let journal = self.session.journal.lock().await;
        self.projector
            .refresh_checked(&journal, messages, &model.model)
            .map_err(|error| ara_agent::ActualProviderError::Callback(error.to_string()))?;
        drop(journal);
        self.projector
            .convert_current_actual(messages, &model.model, values)
    }
    async fn transform_provider_context_actual(
        &self,
        context: ara_agent::ActualProviderContext,
        model: &ara_ai::Model,
        values: &SessionValues,
    ) -> Result<ara_agent::ActualProviderContext, ara_agent::ActualProviderError> {
        self.base
            .transform_provider_context_actual(context, model, values)
            .await
    }
    async fn transform_assistant_graph(
        &self,
        message: &AgentMessage,
        cancel: &CancellationToken,
        values: &SessionValues,
    ) -> Result<(), ara_agent::ActualProviderError> {
        self.base
            .transform_assistant_graph(message, cancel, values)
            .await
    }
    async fn turn_end_actual_context(
        &self,
        context: ara_agent::ActualTurnEndContext<'_>,
        cancel: &CancellationToken,
    ) -> Result<Vec<AgentMessage>, ara_agent::ActualProviderError> {
        self.base.turn_end_actual_context(context, cancel).await
    }
    fn supports_actual_turn_end(&self) -> bool {
        self.base.supports_actual_turn_end()
    }
    async fn turn_end_actual(
        &self,
        message: &AgentMessage,
        results: &[AgentMessage],
        cancel: &CancellationToken,
        values: &SessionValues,
    ) -> Result<Vec<AgentMessage>, ara_agent::ActualProviderError> {
        self.base
            .turn_end_actual(message, results, cancel, values)
            .await
    }
    async fn aside_messages(
        &self,
    ) -> std::result::Result<Vec<ara_agent::AsideMessage>, ara_agent::AsideError> {
        let mut entries=self.base.aside_messages().await?;
        // Preserve passive/base producer order and callbacks. The ordinary
        // same edit owner already drained its batch; this fallback retains the
        // authoritative current Session owner for other Host wrappers.
        entries.extend(self.session.edit_session().map(|owner|owner.asides(self.session.projector.clone(),self.session.native_session.clone())).unwrap_or_default());
        Ok(entries)
    }
    async fn before_yield(&self) -> std::result::Result<(), ara_agent::AsideError> {
        self.base.before_yield().await
    }
    async fn steering_raw_messages(&self) -> Vec<AgentMessage> {
        self.base.steering_raw_messages().await
    }
    async fn follow_up_raw_messages(&self) -> Vec<AgentMessage> {
        self.base.follow_up_raw_messages().await
    }
    async fn turn_end_raw_messages_with_context(
        &self,
        context: ara_agent::TurnEndContext<'_>,
        cancel: &CancellationToken,
    ) -> Vec<AgentMessage> {
        self.base
            .turn_end_raw_messages_with_context(context, cancel)
            .await
    }

    fn execution_snapshot(&self) -> Option<ExecutionSnapshot> {
        let mut snapshot = self.snapshot.read().unwrap_or_else(|e| e.into_inner()).clone();
        // Core admits this snapshot over the static config. Keep the final
        // reminder's yield pin scoped to its actual Run, including fresh reads.
        if self.forced_child_yield {
            if let Some(model) = &mut snapshot.model {
                model.tool_choice = Some(ara_ai::ToolChoice::Function("yield".into()));
            }
        }
        Some(snapshot)
    }

    async fn before_queued_input_dequeue(
        &self,
        cancel: &CancellationToken,
    ) -> Result<(), ara_agent::AdmissionError> {
        self.base.before_queued_input_dequeue(cancel).await
    }

    async fn before_model_call(
        &self,
        snapshot: ExecutionSnapshot,
        cancel: &CancellationToken,
    ) -> Result<ExecutionSnapshot, ara_agent::AdmissionError> {
        self.base.before_model_call(snapshot, cancel).await
    }

    async fn before_queued_input_dequeue_with_context(
        &self,
        snapshot: ExecutionSnapshot,
        messages: Vec<Message>,
        cancel: &CancellationToken,
    ) -> Result<(), ara_agent::AdmissionError> {
        self.base
            .before_queued_input_dequeue_with_context(snapshot.clone(), messages.clone(), cancel)
            .await?;
        if let Some(binding) = &self.admission {
            binding
                .request(
                    false,
                    snapshot,
                    messages.into_iter().map(AgentMessage::from).collect(),
                    cancel,
                )
                .await?;
        }
        Ok(())
    }

    async fn before_model_call_with_context(
        &self,
        snapshot: ExecutionSnapshot,
        messages: Vec<Message>,
        cancel: &CancellationToken,
    ) -> Result<ExecutionSnapshot, ara_agent::AdmissionError> {
        let snapshot = self
            .base
            .before_model_call_with_context(snapshot, messages.clone(), cancel)
            .await?;
        match &self.admission {
            Some(binding) => {
                binding
                    .request(
                        true,
                        snapshot,
                        messages.into_iter().map(AgentMessage::from).collect(),
                        cancel,
                    )
                    .await
            }
            None => Ok(snapshot),
        }
    }

    async fn before_tool_call(
        &self,
        call: &ara_ai::ToolCall,
        args: &ara_ai::JsonObject,
        cancel: &CancellationToken,
    ) -> ToolDecision {
        self.base.before_tool_call(call, args, cancel).await
    }

    async fn before_tool_dispatch(
        &self,
        context: &mut ara_agent::BeforeToolCallContext<'_>,
        cancel: &CancellationToken,
    ) -> Result<ara_agent::BeforeToolCallResult, ara_agent::ToolHookError> {
        self.base.before_tool_dispatch(context, cancel).await
    }
    async fn after_tool_call(
        &self,
        context: ara_agent::AfterToolCallContext<'_>,
        cancel: &CancellationToken,
    ) -> Result<Option<ara_agent::AfterToolCallResult>, ara_agent::ToolHookError> {
        self.base.after_tool_call(context, cancel).await
    }
    async fn transform_assistant_message(
        &self,
        message: &mut AssistantMessage,
        cancel: &CancellationToken,
    ) -> Result<(), ara_agent::ToolHookError> {
        self.base.transform_assistant_message(message, cancel).await
    }
    fn resolve_fallback_tool(&self, name: &str) -> Option<Arc<dyn ara_agent::AgentTool>> {
        self.base.resolve_fallback_tool(name)
    }
    fn transform_tool_call_arguments(
        &self,
        args: ara_ai::JsonObject,
        name: &str,
    ) -> Result<ara_ai::JsonObject, ara_agent::ToolHookError> {
        self.base.transform_tool_call_arguments(args, name)
    }
    fn steering_interrupt_binding(&self) -> Option<ara_agent::SteeringInterruptBinding> {
        self.base.steering_interrupt_binding()
    }
    fn irc_interrupt_port(&self) -> Option<Arc<dyn ara_agent::IrcInterruptPeek>> {
        self.base.irc_interrupt_port()
    }

    async fn steering_inputs(&self) -> Vec<AgentInput> {
        self.base.steering_inputs().await
    }
    async fn follow_up_inputs(&self) -> Vec<AgentInput> {
        self.base.follow_up_inputs().await
    }
    async fn transform_provider_context(
        &self,
        context: ara_ai::Context,
        model: &ara_ai::Model,
    ) -> ara_ai::Context {
        self.base.transform_provider_context(context, model).await
    }

    async fn turn_end_inputs(
        &self,
        message: &AssistantMessage,
        results: &[ara_ai::ToolResultMessage],
        cancel: &CancellationToken,
    ) -> Vec<AgentInput> {
        self.base.turn_end_inputs(message, results, cancel).await
    }
    async fn turn_end_inputs_with_context(
        &self,
        context: ara_agent::TurnEndContext<'_>,
        cancel: &CancellationToken,
    ) -> Vec<AgentInput> {
        self.base
            .turn_end_inputs_with_context(context, cancel)
            .await
    }
}

/// Per-Run adapter; the detector itself belongs to the long-lived RPC Host.
/// Core owns insertion into its transcript and awaits the native input sink
/// before it admits another model call, a deadline, or user steering.
struct ToolLoopHooks {
    base: Arc<dyn LoopHooks>,
    native_session: Option<Arc<ara_cli::native_factory::PreparedNativeSession>>,
    state: Arc<Mutex<ToolLoopState>>,
    settings: ToolLoopGuardSettings,
    output: Output,
    session_id: Value,
    run_cancel: CancellationToken,
}

#[async_trait]
impl LoopHooks for ToolLoopHooks {
    fn actual_tool_source(
        &self,
        values: &SessionValues,
    ) -> Result<Option<Arc<dyn ara_agent::ActualToolSource>>, ara_agent::ActualProviderError> {
        self.base.actual_tool_source(values)
    }
    fn supports_actual_tool_hooks(&self) -> bool {
        self.base.supports_actual_tool_hooks()
    }
    async fn before_tool_dispatch_actual(
        &self,
        context: ara_agent::ActualBeforeToolCallContext<'_>,
        cancel: &CancellationToken,
    ) -> Result<Option<ara_rpc::value_heap::ValueHandle>, ara_agent::ActualProviderError> {
        self.base.before_tool_dispatch_actual(context, cancel).await
    }
    async fn before_tool_call_actual(
        &self,
        context: ara_agent::ActualBeforeToolCallContext<'_>,
        cancel: &CancellationToken,
    ) -> Result<ara_agent::ActualToolDecision, ara_agent::ActualProviderError> {
        self.base.before_tool_call_actual(context, cancel).await
    }
    async fn after_tool_call_actual(
        &self,
        context: ara_agent::ActualAfterToolCallContext<'_>,
        cancel: &CancellationToken,
    ) -> Result<Option<ara_rpc::value_heap::ValueHandle>, ara_agent::ActualProviderError> {
        self.base.after_tool_call_actual(context, cancel).await
    }
    fn transform_tool_call_arguments_actual(
        &self,
        args: ara_rpc::value_heap::ValueHandle,
        name: &ara_rpc::WireString,
        values: &SessionValues,
    ) -> Result<ara_rpc::value_heap::ValueHandle, ara_agent::ActualProviderError> {
        self.base
            .transform_tool_call_arguments_actual(args, name, values)
    }
    fn get_tool_context_actual(
        &self,
        context: &ara_agent::ActualToolExecutionContext,
        values: &SessionValues,
    ) -> Result<ara_rpc::value_heap::ValueHandle, ara_agent::ActualProviderError> {
        let fresh = self.base.get_tool_context_actual(context, values)?;
        if let Some(prepared) = &self.native_session {
            prepared
                .require_tool_hooks()?
                .bind_run_context(values, &fresh)?;
        }
        Ok(fresh)
    }
    fn tool_error_message_actual(
        &self,
        error: &ara_rpc::value_heap::ValueError,
        values: &SessionValues,
    ) -> Result<ara_rpc::value_heap::ValueHandle, ara_agent::ActualProviderError> {
        self.base.tool_error_message_actual(error, values)
    }
    fn sanitize_tool_result_text_actual(
        &self,
        text: &ara_rpc::value_heap::ValueHandle,
        values: &SessionValues,
    ) -> Result<ara_rpc::value_heap::ValueHandle, ara_agent::ActualProviderError> {
        self.base.sanitize_tool_result_text_actual(text, values)
    }
    fn soft_tool_satisfies_actual(
        &self,
        requirement: &ara_agent::soft_tool_requirement::SoftToolRequirement,
        call: &ara_rpc::value_heap::ValueHandle,
        values: &SessionValues,
    ) -> Result<bool, ara_agent::ActualProviderError> {
        self.base
            .soft_tool_satisfies_actual(requirement, call, values)
    }
    fn final_provider_gate_binding(&self) -> Option<ara_agent::FinalProviderGateBinding> {
        self.base.final_provider_gate_binding()
    }
    fn on_final_provider_gate_rejected(
        &self,
    ) -> std::result::Result<(), ara_agent::FinalProviderGateError> {
        self.base.on_final_provider_gate_rejected()
    }
    fn select_tool_choice_directive(
        &self,
        active_tools: &[Arc<dyn ara_agent::AgentTool>],
        admitted_static_choice: Option<&ara_ai::ToolChoice>,
    ) -> std::result::Result<ara_agent::ToolChoiceSelection, ara_agent::ToolDirectiveError> {
        self.base
            .select_tool_choice_directive(active_tools, admitted_static_choice)
    }
    fn select_tool_choice_directive_full(
        &self,
        active_tools: &[Arc<dyn ara_agent::AgentTool>],
        admitted_static_choice: Option<&ara_ai::ToolChoice>,
    ) -> std::result::Result<
        ara_agent::soft_tool_requirement::ToolChoiceDirectiveSelection,
        ara_agent::ToolDirectiveError,
    > {
        self.base
            .select_tool_choice_directive_full(active_tools, admitted_static_choice)
    }
    fn soft_tool_requirement_state(
        &self,
    ) -> Option<ara_agent::soft_tool_requirement::SoftToolRequirementState> {
        self.base.soft_tool_requirement_state()
    }

    fn on_deferred_tool_choice_unavailable(
        &self,
    ) -> std::result::Result<(), ara_agent::ToolDirectiveError> {
        self.base.on_deferred_tool_choice_unavailable()
    }
    fn provider_context_binding(&self) -> Option<ara_agent::ProviderContextBinding> {
        self.base.provider_context_binding()
    }
    fn set_append_only_context(
        &self,
        manager: Option<ara_agent::SharedAppendOnlyContextManager>,
    ) -> std::result::Result<(), ara_agent::ProviderContextError> {
        self.base.set_append_only_context(manager)
    }

    fn queued_input_pending(&self, kind: ara_agent::QueueInputKind) -> Option<bool> {
        self.base.queued_input_pending(kind)
    }
    fn context_admission_enabled(&self) -> bool {
        self.base.context_admission_enabled()
    }
    fn execution_snapshot(&self) -> Option<ExecutionSnapshot> {
        self.base.execution_snapshot()
    }
    async fn before_queued_input_dequeue(
        &self,
        cancel: &CancellationToken,
    ) -> Result<(), ara_agent::AdmissionError> {
        self.base.before_queued_input_dequeue(cancel).await
    }
    async fn before_model_call(
        &self,
        snapshot: ExecutionSnapshot,
        cancel: &CancellationToken,
    ) -> Result<ExecutionSnapshot, ara_agent::AdmissionError> {
        self.base.before_model_call(snapshot, cancel).await
    }
    async fn before_queued_input_dequeue_with_context(
        &self,
        snapshot: ExecutionSnapshot,
        messages: Vec<Message>,
        cancel: &CancellationToken,
    ) -> Result<(), ara_agent::AdmissionError> {
        self.base
            .before_queued_input_dequeue_with_context(snapshot, messages, cancel)
            .await
    }
    async fn before_model_call_with_context(
        &self,
        snapshot: ExecutionSnapshot,
        messages: Vec<Message>,
        cancel: &CancellationToken,
    ) -> Result<ExecutionSnapshot, ara_agent::AdmissionError> {
        self.base
            .before_model_call_with_context(snapshot, messages, cancel)
            .await
    }
    async fn before_tool_call(
        &self,
        call: &ara_ai::ToolCall,
        args: &ara_ai::JsonObject,
        cancel: &CancellationToken,
    ) -> ToolDecision {
        self.base.before_tool_call(call, args, cancel).await
    }
    async fn before_tool_dispatch(
        &self,
        context: &mut ara_agent::BeforeToolCallContext<'_>,
        cancel: &CancellationToken,
    ) -> Result<ara_agent::BeforeToolCallResult, ara_agent::ToolHookError> {
        self.base.before_tool_dispatch(context, cancel).await
    }
    async fn after_tool_call(
        &self,
        context: ara_agent::AfterToolCallContext<'_>,
        cancel: &CancellationToken,
    ) -> Result<Option<ara_agent::AfterToolCallResult>, ara_agent::ToolHookError> {
        self.base.after_tool_call(context, cancel).await
    }
    async fn transform_assistant_message(
        &self,
        message: &mut AssistantMessage,
        cancel: &CancellationToken,
    ) -> Result<(), ara_agent::ToolHookError> {
        self.base.transform_assistant_message(message, cancel).await
    }
    fn resolve_fallback_tool(&self, name: &str) -> Option<Arc<dyn ara_agent::AgentTool>> {
        self.base.resolve_fallback_tool(name)
    }
    fn transform_tool_call_arguments(
        &self,
        args: ara_ai::JsonObject,
        name: &str,
    ) -> Result<ara_ai::JsonObject, ara_agent::ToolHookError> {
        self.base.transform_tool_call_arguments(args, name)
    }
    fn steering_interrupt_binding(&self) -> Option<ara_agent::SteeringInterruptBinding> {
        self.base.steering_interrupt_binding()
    }
    fn irc_interrupt_port(&self) -> Option<Arc<dyn ara_agent::IrcInterruptPeek>> {
        self.base.irc_interrupt_port()
    }

    async fn steering_inputs(&self) -> Vec<AgentInput> {
        self.base.steering_inputs().await
    }
    async fn follow_up_inputs(&self) -> Vec<AgentInput> {
        self.base.follow_up_inputs().await
    }
    async fn transform_provider_context(
        &self,
        context: ara_ai::Context,
        model: &ara_ai::Model,
    ) -> ara_ai::Context {
        self.base.transform_provider_context(context, model).await
    }
    async fn before_queued_input_dequeue_with_raw_context(
        &self,
        snapshot: ExecutionSnapshot,
        messages: Vec<AgentMessage>,
        cancel: &CancellationToken,
    ) -> Result<(), ara_agent::AdmissionError> {
        self.base
            .before_queued_input_dequeue_with_raw_context(snapshot, messages, cancel)
            .await
    }
    async fn before_model_call_with_raw_context(
        &self,
        snapshot: ExecutionSnapshot,
        messages: Vec<AgentMessage>,
        cancel: &CancellationToken,
    ) -> Result<ExecutionSnapshot, ara_agent::AdmissionError> {
        self.base
            .before_model_call_with_raw_context(snapshot, messages, cancel)
            .await
    }
    async fn transform_agent_context(
        &self,
        live: &mut Vec<AgentMessage>,
        cancel: &CancellationToken,
    ) -> Result<Option<Vec<AgentMessage>>, RawContextError> {
        self.base.transform_agent_context(live, cancel).await
    }
    async fn convert_agent_messages_to_llm(
        &self,
        messages: &[AgentMessage],
    ) -> Result<Vec<Message>, RawContextError> {
        self.base.convert_agent_messages_to_llm(messages).await
    }
    async fn convert_agent_messages_to_llm_actual(
        &self,
        messages: &[AgentMessage],
        values: &SessionValues,
    ) -> Result<Vec<ara_rpc::value_heap::ValueHandle>, ara_agent::ActualProviderError> {
        self.base
            .convert_agent_messages_to_llm_actual(messages, values)
            .await
    }
    async fn transform_provider_context_actual(
        &self,
        context: ara_agent::ActualProviderContext,
        model: &ara_ai::Model,
        values: &SessionValues,
    ) -> Result<ara_agent::ActualProviderContext, ara_agent::ActualProviderError> {
        self.base
            .transform_provider_context_actual(context, model, values)
            .await
    }
    async fn transform_assistant_graph(
        &self,
        message: &AgentMessage,
        cancel: &CancellationToken,
        values: &SessionValues,
    ) -> Result<(), ara_agent::ActualProviderError> {
        self.base
            .transform_assistant_graph(message, cancel, values)
            .await
    }
    fn supports_actual_turn_end(&self) -> bool {
        self.base.supports_actual_turn_end()
            && (!self.settings.enabled
                || self
                    .native_session
                    .as_ref()
                    .is_some_and(|native| native.logger().is_some()))
    }
    async fn turn_end_actual_context(
        &self,
        context: ara_agent::ActualTurnEndContext<'_>,
        cancel: &CancellationToken,
    ) -> Result<Vec<AgentMessage>, ara_agent::ActualProviderError> {
        let message = context.assistant_message;
        let results = context.tool_results;
        let values = context.values;
        let stop_reason = context.stop_reason;
        let inputs = self.base.turn_end_actual_context(context, cancel).await?;
        self.append_loop_actual(
            inputs,
            message,
            results,
            cancel,
            values,
            native_loop::StopObservation::Observed(stop_reason),
            native_loop::NoticeBoundary::PublicRpc,
        )
    }
    async fn turn_end_actual(
        &self,
        message: &AgentMessage,
        results: &[AgentMessage],
        cancel: &CancellationToken,
        values: &SessionValues,
    ) -> Result<Vec<AgentMessage>, ara_agent::ActualProviderError> {
        let inputs = self
            .base
            .turn_end_actual(message, results, cancel, values)
            .await?;
        self.append_loop_actual(
            inputs,
            message,
            results,
            cancel,
            values,
            native_loop::StopObservation::CurrentGraph,
            native_loop::NoticeBoundary::Native,
        )
    }
    async fn aside_messages(
        &self,
    ) -> std::result::Result<Vec<ara_agent::AsideMessage>, ara_agent::AsideError> {
        self.base.aside_messages().await
    }
    async fn before_yield(&self) -> std::result::Result<(), ara_agent::AsideError> {
        self.base.before_yield().await
    }
    async fn steering_raw_messages(&self) -> Vec<AgentMessage> {
        self.base.steering_raw_messages().await
    }
    async fn follow_up_raw_messages(&self) -> Vec<AgentMessage> {
        self.base.follow_up_raw_messages().await
    }
    async fn turn_end_raw_messages_with_context(
        &self,
        context: ara_agent::TurnEndContext<'_>,
        cancel: &CancellationToken,
    ) -> Vec<AgentMessage> {
        let message = context.message;
        let results = context.tool_results;
        let inputs = self
            .base
            .turn_end_raw_messages_with_context(context, cancel)
            .await;
        self.append_loop_raw(inputs, message, results, cancel)
    }
    async fn turn_end_inputs(
        &self,
        message: &AssistantMessage,
        results: &[ara_ai::ToolResultMessage],
        cancel: &CancellationToken,
    ) -> Vec<AgentInput> {
        let inputs = self.base.turn_end_inputs(message, results, cancel).await;
        self.append_loop_inputs(inputs, message, results, cancel)
    }
    async fn turn_end_inputs_with_context(
        &self,
        context: ara_agent::TurnEndContext<'_>,
        cancel: &CancellationToken,
    ) -> Vec<AgentInput> {
        let message = context.message;
        let results = context.tool_results;
        let inputs = self
            .base
            .turn_end_inputs_with_context(context, cancel)
            .await;
        self.append_loop_inputs(inputs, message, results, cancel)
    }
}

impl ToolLoopHooks {
    fn append_loop_inputs(
        &self,
        mut inputs: Vec<AgentInput>,
        message: &AssistantMessage,
        results: &[ara_ai::ToolResultMessage],
        cancel: &CancellationToken,
    ) -> Vec<AgentInput> {
        if cancel.is_cancelled()
            || matches!(
                message.stop_reason,
                ara_ai::StopReason::Error | ara_ai::StopReason::Aborted
            )
        {
            return inputs;
        }
        let detection = {
            let mut state = self.state.lock().unwrap();
            state.configure(self.settings.clone());
            state.record_turn(message, results)
        };
        if let Some(detection) = detection {
            let projected = LoopGuardNotice::tool_call_loop(&detection).and_then(|notice| {
                let model = notice.try_model_message()?;
                Ok(AgentInput {
                    model,
                    provenance: Some(Arc::new(notice.event_message())),
                })
            });
            match projected {
                Ok(input) => inputs.push(input),
                Err(error) => {
                    // Native JavaScript truncation can leave a lone UTF-16
                    // surrogate. Retain the exact wire receipt, refuse today's
                    // UTF-8 context projection and cancel this Run; never replay.
                    let mut receipt = wire(
                        json!({"type":"notice","level":"error","source":"loop-guard",
                        "sessionId":self.session_id,"message":error.to_string()}),
                    );
                    receipt.insert("details", detection.to_wire_value());
                    self.output.send(OutputItem::Frame(receipt));
                    cancel.cancel();
                    // Core owns a child token. Cancel the Host's retained Run
                    // token too so its existing retry veto cannot re-dispatch.
                    self.run_cancel.cancel();
                }
            }
        }
        inputs
    }
    fn append_loop_raw(
        &self,
        mut inputs: Vec<AgentMessage>,
        message: &AssistantMessage,
        results: &[ara_ai::ToolResultMessage],
        cancel: &CancellationToken,
    ) -> Vec<AgentMessage> {
        if cancel.is_cancelled()
            || matches!(
                message.stop_reason,
                ara_ai::StopReason::Error | ara_ai::StopReason::Aborted
            )
        {
            return inputs;
        }
        let detection = {
            let mut state = self.state.lock().unwrap();
            state.configure(self.settings.clone());
            state.record_turn(message, results)
        };
        if let Some(detection) = detection {
            let projected = LoopGuardNotice::tool_call_loop(&detection).and_then(|notice| {
                let model = notice.try_model_message()?;
                let _ = model;
                ara_cli::agent_message_projection::raw_custom_input(notice.event_message())
                    .map_err(std::io::Error::other)
            });
            match projected {
                Ok(input) => inputs.push(input),
                Err(error) => {
                    // Native JavaScript truncation can leave a lone UTF-16
                    // surrogate. Retain the exact wire receipt, refuse today's
                    // UTF-8 context projection and cancel this Run; never replay.
                    let mut receipt = wire(
                        json!({"type":"notice","level":"error","source":"loop-guard",
                        "sessionId":self.session_id,"message":error.to_string()}),
                    );
                    receipt.insert("details", detection.to_wire_value());
                    self.output.send(OutputItem::Frame(receipt));
                    cancel.cancel();
                    // Core owns a child token. Cancel the Host's retained Run
                    // token too so its existing retry veto cannot re-dispatch.
                    self.run_cancel.cancel();
                }
            }
        }
        inputs
    }
}

use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::mpsc,
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

enum OutputItem {
    Frame(WireValue),
    Negotiate(WireValue),
}

#[derive(Clone)]
struct Output(mpsc::UnboundedSender<OutputItem>);

fn wire(value: Value) -> WireValue {
    // serde_json's output is valid JSON; the transport preserves its JS wire
    // representation. Inbound correlation IDs never pass through serde_json.
    WireValue::parse(&value.to_string()).expect("serialized JSON")
}

pub(super) fn snapcompact_public_response(
    prepared: &ara_cli::snapcompact::PreparedSnapcompact,
) -> Result<WireValue> {
    let mut response = wire(json!({"summary":prepared.summary.summary,
        "shortSummary":prepared.summary.short_summary,"method":"snapcompact",
        "firstKeptEntryId":prepared.result.first_kept_entry_id,
        "tokensBefore":prepared.result.tokens_before,"tokensAfter":prepared.tokens_after}));
    if let Some(preserve) =
        ara_snapcompact::strip_preserved_archive_utf16(prepared.result.preserve_data.as_deref())?
    {
        response.insert("preserveData", WireValue::parse(preserve.get())?);
    }
    Ok(response)
}

impl Output {
    fn frame(&self, frame: Value) {
        self.frame_wire(wire(frame));
    }

    fn frame_wire(&self, frame: WireValue) {
        self.send(OutputItem::Frame(frame));
    }

    fn send(&self, frame: OutputItem) {
        // The output owner cancels the connection on failure. There is no await
        // here: a stalled peer cannot stop input/control or journal receipts.
        let _ = self.0.send(frame);
    }

    fn response(&self, command: &Command, data: Option<Value>, error: Option<String>) {
        self.response_wire(command, data.map(wire), error, None);
    }

    fn response_wire(
        &self,
        command: &Command,
        data: Option<WireValue>,
        error: Option<String>,
        code: Option<&str>,
    ) {
        let mut response =
            json!({"type":"response", "command":command.kind, "success":error.is_none()});
        if let Some(error) = error {
            response["error"] = json!(error);
            if let Some(code) = code {
                response["code"] = json!(code);
            }
        }
        let mut response = wire(response);
        if response.get("success") == Some(&WireValue::Bool(true))
            && let Some(data) = data
        {
            response.insert("data", data);
        }
        if let (Some(id), WireValue::Object(fields)) = (&command.id, &mut response) {
            fields.push(("id".into(), id.clone()));
        }
        let negotiated = command.kind == "negotiate_protocol"
            && response.get("success") == Some(&WireValue::Bool(true));
        self.send(if negotiated {
            OutputItem::Negotiate(response)
        } else {
            OutputItem::Frame(response)
        });
    }

    fn prompt_skipped(&self, command: &Command) {
        let mut result = wire(json!({"type":"prompt_result","agentInvoked":false}));
        if let (Some(id), WireValue::Object(fields)) = (&command.id, &mut result) {
            fields.push(("id".into(), id.clone()));
        }
        self.send(OutputItem::Frame(result));
    }
}

async fn write_output<W: AsyncWrite + Unpin>(
    writer: W,
    mut rx: mpsc::UnboundedReceiver<OutputItem>,
    cancel: CancellationToken,
) -> Result<()> {
    let mut writer = RpcOutput::new(writer);
    while let Some(item) = rx.recv().await {
        let (frame, negotiate) = match item {
            OutputItem::Frame(frame) => (frame, false),
            OutputItem::Negotiate(frame) => (frame, true),
        };
        if let Err(error) = writer.write_frame(&frame).await {
            cancel.cancel();
            return Err(error).context("writing RPC output");
        }
        if negotiate {
            writer.set_protocol_version(2)?;
        }
    }
    Ok(())
}

struct Command {
    id: Option<WireValue>,
    kind: String,
    frame: WireValue,
}

// These typed inputs reuse the owning Native Host actor. They are not wire
// protocol frames and never pass through the ARA RPC parser/encoder.
pub(super) enum NativeAction {
    State,
    Messages,
    Prompt {
        text: String,
        images: Vec<ara_ai::ImageContent>,
    },
    Abort,
    RefreshPlan {
        decision: Arc<NativePlanRefresh>,
    },
    WaitIdle,
    PeerControl {operation:String,init:Option<Value>,target:Option<String>,context:Option<String>,structured:bool},
    Switch {path:PathBuf},
    ChildInstall { session_id:String, ticket:ara_cli::child_contract::InstallTicket, snapshot:ara_cli::child_contract::ContractSnapshot },
    ChildPrompt { session_id:String, text:Option<String>, turn:Arc<super::child_host::ChildTurn> },
}
pub(super) enum NativeChildAction {
    Install {session_id:String,ticket:ara_cli::child_contract::InstallTicket,snapshot:ara_cli::child_contract::ContractSnapshot},
    Prompt {session_id:String,text:Option<String>,turn:Arc<super::child_host::ChildTurn>},
}
struct NativeRequest {
    action: NativeAction,
    reply: tokio::sync::oneshot::Sender<Result<Value>>,
}
pub(super) struct NativePlanRefresh {
    state: Mutex<NativePlanRefreshState>,
}
#[derive(Default)]
struct NativeCancelReceipt {
    requested: std::sync::atomic::AtomicBool,
    aborted: std::sync::atomic::AtomicBool,
    unknown_effect: Mutex<Option<String>>,
}
enum NativePlanRefreshState {
    Pending,
    Cancelled(String),
    Committed(std::result::Result<(), String>),
}
impl NativePlanRefresh {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(NativePlanRefreshState::Pending),
        })
    }
    fn cancel_or_committed(&self, reason: String) -> std::result::Result<(), String> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match &*state {
            NativePlanRefreshState::Committed(result) => result.clone(),
            NativePlanRefreshState::Cancelled(reason) => Err(reason.clone()),
            NativePlanRefreshState::Pending => {
                *state = NativePlanRefreshState::Cancelled(reason.clone());
                Err(reason)
            }
        }
    }
    fn reject_before_publication(&self, error: String) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(&*state, NativePlanRefreshState::Pending) {
            *state = NativePlanRefreshState::Committed(Err(error));
        }
    }
}
#[derive(Clone)]
pub(super) struct NativeSessionHandle {
    sender: mpsc::UnboundedSender<NativeRequest>,
    connection: CancellationToken,
    refresh_epoch: tokio::sync::watch::Sender<u64>,
    refresh_blocked: Arc<std::sync::atomic::AtomicBool>,
    failure: Arc<Mutex<Option<String>>>,
    cancel_receipt: Arc<NativeCancelReceipt>,
}
impl NativeSessionHandle {
    pub fn admit(
        &self,
        action: NativeAction,
    ) -> Result<tokio::sync::oneshot::Receiver<Result<Value>>> {
        if matches!(&action, NativeAction::Abort) {
            self.cancel_receipt
                .requested
                .store(true, std::sync::atomic::Ordering::Release);
            self.refresh_blocked
                .store(true, std::sync::atomic::Ordering::Release);
            self.refresh_epoch
                .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
        } else if matches!(&action, NativeAction::Prompt { .. } | NativeAction::ChildPrompt {..}) {
            self.permit_policy_refresh();
            *self.failure.lock().unwrap() = None;
            self.cancel_receipt
                .requested
                .store(false, std::sync::atomic::Ordering::Release);
            self.cancel_receipt
                .aborted
                .store(false, std::sync::atomic::Ordering::Release);
            *self.cancel_receipt.unknown_effect.lock().unwrap() = None;
        }
        let (reply, received) = tokio::sync::oneshot::channel();
        self.sender
            .send(NativeRequest { action, reply })
            .map_err(|_| anyhow::anyhow!("Native Session closed"))?;
        Ok(received)
    }
    pub async fn call(&self, action: NativeAction) -> Result<Value> {
        let wait_idle = matches!(&action, NativeAction::WaitIdle);
        let received = self.admit(action)?;
        let mut value = received
            .await
            .context("Native Session response owner closed")??;
        // Ordered output processes terminal failure before the idle receipt.
        if wait_idle {
            let cancelled = self
                .cancel_receipt
                .requested
                .load(std::sync::atomic::Ordering::Acquire)
                || self
                    .cancel_receipt
                    .aborted
                    .load(std::sync::atomic::Ordering::Acquire);
            if cancelled
                && let Some(error) = self.cancel_receipt.unknown_effect.lock().unwrap().clone()
            {
                bail!(error);
            }
            if let Some(error) = self.failure.lock().unwrap().clone() {
                if !(cancelled && error == "Request was aborted") {
                    bail!(error);
                }
            }
            if cancelled {
                value["_nativeCancelled"] = json!(true);
            }
        }
        Ok(value)
    }
    pub fn weak_refresh(&self) -> super::acp_plan::PlanRefresh {
        let sender = self.sender.downgrade();
        let refresh_epoch = self.refresh_epoch.clone();
        let connection = self.connection.clone();
        let blocked = self.refresh_blocked.clone();
        Arc::new(move || {
            let sender = sender.clone();
            let mut epoch = refresh_epoch.subscribe();
            let connection = connection.clone();
            let blocked = blocked.clone();
            Box::pin(async move {
                if blocked.load(std::sync::atomic::Ordering::Acquire) {
                    return Err(
                        "Native Run cancellation has closed plan refresh admission".to_owned()
                    );
                }
                let sender = sender
                    .upgrade()
                    .ok_or_else(|| "Native Session closed".to_owned())?;
                let (reply, received) = tokio::sync::oneshot::channel();
                let decision = NativePlanRefresh::new();
                sender
                    .send(NativeRequest {
                        action: NativeAction::RefreshPlan {
                            decision: decision.clone(),
                        },
                        reply,
                    })
                    .map_err(|_| "Native Session closed".to_owned())?;
                // A tool can propose while the serial actor is about to join
                // that same Run. Release this pure policy callback on abort;
                // admitted external filesystem requests keep their own owner.
                tokio::select! {
                    biased;
                    _ = connection.cancelled() => decision.cancel_or_committed("Native Session closed during plan refresh".to_owned()),
                    _ = epoch.changed() => decision.cancel_or_committed("Native Run cancelled during plan refresh".to_owned()),
                    result = received => match result {
                        Ok(Ok(_)) => decision.cancel_or_committed("Native plan refresh lacks publication receipt".to_owned()),
                        Ok(Err(error)) => decision.cancel_or_committed(error.to_string()),
                        Err(_) => decision.cancel_or_committed("Native Session response closed".to_owned()),
                    },
                }
            })
        })
    }
    pub fn close_admission(&self) {
        self.refresh_blocked
            .store(true, std::sync::atomic::Ordering::Release);
        self.refresh_epoch
            .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
        self.connection.cancel();
    }
    pub fn permit_policy_refresh(&self) {
        self.refresh_blocked
            .store(false, std::sync::atomic::Ordering::Release);
    }
    pub fn closed_signal(&self) -> CancellationToken {
        self.connection.clone()
    }
}
pub(super) struct NativeSessionRuntime {
    pub handle: NativeSessionHandle,
    task: JoinHandle<Result<i32>>,
}
impl NativeSessionRuntime {
    pub async fn close(self) -> Result<i32> {
        self.handle.close_admission();
        self.task.await.context("Native Session actor join")?
    }
}
type NativeFrameSink = Arc<dyn Fn(WireValue) + Send + Sync>;
struct NativeIo {
    input: mpsc::UnboundedReceiver<NativeRequest>,
    pending:
        Arc<Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<Result<Value>>>>>,
    frames: NativeFrameSink,
    connection: CancellationToken,
    failure: Arc<Mutex<Option<String>>>,
    cancel_receipt: Arc<NativeCancelReceipt>,
}
enum OwnedInput {
    Command(Command),
    NativeIdle(Command),
    NativeRefresh(Command, Arc<NativePlanRefresh>),
    NativeChild(Command,NativeChildAction),
}

impl NativeIo {
    fn command(&self, request: NativeRequest) -> OwnedInput {
        let id = uuid::Uuid::now_v7().to_string();
        let mut frame = json!({"id":id});
        let mut refresh = None;
        let action=match request.action {
            NativeAction::ChildInstall{session_id,ticket,snapshot}=>{
                frame["type"]=json!("__native_child_install");self.pending.lock().unwrap().insert(id,request.reply);
                return OwnedInput::NativeChild(Command::new(wire(frame)),NativeChildAction::Install{session_id,ticket,snapshot});
            }
            NativeAction::ChildPrompt{session_id,text,turn}=>{
                frame["type"]=json!("__native_child_prompt");self.pending.lock().unwrap().insert(id,request.reply);
                return OwnedInput::NativeChild(Command::new(wire(frame)),NativeChildAction::Prompt{session_id,text,turn});
            }
            action=>action,
        };
        let (kind, special) = match action {
            NativeAction::ChildInstall{..}|NativeAction::ChildPrompt{..}=>unreachable!("typed child action already extracted"),
            NativeAction::State => ("get_state", 0),
            NativeAction::Messages => ("get_messages", 0),
            NativeAction::Abort => ("abort", 0),
            NativeAction::WaitIdle => ("__native_wait_idle", 1),
            NativeAction::PeerControl{operation,init,target,context,structured}=>{frame["operation"]=json!(operation);if let Some(init)=init{frame["init"]=init;}if let Some(target)=target{frame["target"]=json!(target);}if let Some(context)=context{frame["context"]=json!(context);}frame["structured"]=json!(structured);("peer_control",0)},
            NativeAction::Switch{path}=>{frame["sessionPath"]=json!(path);("switch_session",0)},
            NativeAction::RefreshPlan { decision } => {
                refresh = Some(decision);
                ("__native_refresh_plan", 2)
            }
            NativeAction::Prompt { text, images } => {
                frame["message"] = json!(text);
                frame["images"] = json!(images);
                ("abort_and_prompt", 0)
            }
        };
        frame["type"] = json!(kind);
        self.pending.lock().unwrap().insert(id, request.reply);
        let command = Command::new(wire(frame));
        match special {
            1 => OwnedInput::NativeIdle(command),
            2 => OwnedInput::NativeRefresh(command, refresh.expect("Native refresh decision")),
            _ => OwnedInput::Command(command),
        }
    }
}

async fn write_native_output(
    mut rx: mpsc::UnboundedReceiver<OutputItem>,
    io: Arc<NativeOutput>,
) -> Result<()> {
    let mut failure = None;
    while let Some(item) = rx.recv().await {
        let frame = match item {
            OutputItem::Frame(frame) | OutputItem::Negotiate(frame) => frame,
        };
        let kind = frame.get("type").and_then(WireValue::as_string);
        let child_shutdown = io.stop_after_child_shutdown
            && kind.is_some_and(|kind| kind.equals_ascii("session_shutdown"));
        let ascii_field = |value: Option<&WireValue>, expected: &str| {
            value
                .and_then(WireValue::as_string)
                .is_some_and(|value| value.equals_ascii(expected))
        };
        if kind.is_some_and(|kind| kind.equals_ascii("agent_end"))
            && frame
                .get("messages")
                .and_then(WireValue::as_array)
                .and_then(|messages| {
                    messages
                        .iter()
                        .rev()
                        .find(|message| ascii_field(message.get("role"), "assistant"))
                })
                .is_some_and(|message| ascii_field(message.get("stopReason"), "aborted"))
        {
            io.cancel_receipt
                .aborted
                .store(true, std::sync::atomic::Ordering::Release);
        }
        if kind.is_some_and(|kind| kind.equals_ascii("tool_execution_end"))
            && matches!(frame.get("isError"), Some(WireValue::Bool(true)))
        {
            let result = frame.get("result");
            let details = result.and_then(|result| result.get("details"));
            let unknown = ascii_field(
                details.and_then(|details| details.get("executed")),
                "unknown",
            ) || ascii_field(
                details.and_then(|details| details.get("outcome")),
                "unknown",
            ) || result
                .and_then(|result| result.get("content"))
                .and_then(WireValue::as_array)
                .into_iter()
                .flatten()
                .any(|block| {
                    block
                        .get("text")
                        .and_then(WireValue::as_string)
                        .is_some_and(|text| {
                            // Match ASCII diagnostic phrases without decoding or
                            // replacing other UTF-16 code units in the receipt.
                            ["outcome unknown", "delivery unknown"]
                                .iter()
                                .any(|phrase| {
                                    text.units().windows(phrase.len()).any(|units| {
                                        units.iter().zip(phrase.bytes()).all(|(&unit, byte)| {
                                            unit == u16::from(byte)
                                                || unit == u16::from(byte.to_ascii_uppercase())
                                        })
                                    })
                                })
                        })
                });
            if unknown {
                *io.cancel_receipt.unknown_effect.lock().unwrap() = Some(
                    "Run cancelled with an unknown tool effect; inspect its original receipt before continuing".into(),
                );
            }
        }
        if kind.is_some_and(|kind| kind.equals_ascii("response")) {
            let reply = frame
                .get("id")
                .and_then(WireValue::as_string)
                .and_then(|id| id.to_utf8().ok())
                .and_then(|id| io.pending.lock().unwrap().remove(&id));
            if let Some(reply) = reply {
                #[cfg(feature="test-fixture")]
                if std::env::var_os("ARA_TEST_CHILD_DROP_ADMITTED_ACK").is_some()
                    &&frame.get("data").and_then(|data|data.get("admitted"))==Some(&WireValue::Bool(true))
                    &&matches!(frame.get("success"),Some(WireValue::Bool(true))) {
                    drop(reply);(io.frames)(frame);continue;
                }
                // NativeAction has a typed reply contract. Only its requested
                // data is checked here; the event callback retains the full Wire.
                let result = if matches!(frame.get("success"), Some(WireValue::Bool(true))) {
                    let encoded = frame
                        .get("data")
                        .map(WireValue::stringify)
                        .unwrap_or_else(|| "null".into());
                    serde_json::from_str(&encoded).context("Native Host requested data projection")
                } else {
                    match frame
                        .get("error")
                        .and_then(WireValue::as_string)
                        .map(|error| error.to_utf8().context("Native Host error projection"))
                        .transpose()
                    {
                        Ok(error) => Err(anyhow::anyhow!(
                            error.unwrap_or_else(|| "Native Session failed".into())
                        )),
                        Err(error) => {
                            io.connection.cancel();
                            failure = Some(anyhow::anyhow!(error.to_string()));
                            Err(error)
                        }
                    }
                };
                let projection_failure = result
                    .as_ref()
                    .err()
                    .filter(|_| matches!(frame.get("success"), Some(WireValue::Bool(true))))
                    .map(|error| error.to_string());
                let _ = reply.send(result);
                if failure.is_some() {
                    break;
                }
                if let Some(error) = projection_failure {
                    io.connection.cancel();
                    failure = Some(anyhow::anyhow!(error));
                    break;
                }
                continue;
            }
            if matches!(frame.get("success"), Some(WireValue::Bool(false))) {
                match frame
                    .get("error")
                    .and_then(WireValue::as_string)
                    .map(|error| error.to_utf8().context("Native Host error projection"))
                    .transpose()
                {
                    Ok(error) => {
                        *io.failure.lock().unwrap() =
                            Some(error.unwrap_or_else(|| "Native Run failed".into()))
                    }
                    Err(error) => {
                        io.connection.cancel();
                        failure = Some(error);
                        break;
                    }
                }
            }
        }
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (io.frames)(frame))).is_err() {
            io.connection.cancel();
            failure = Some(anyhow::anyhow!("Native Host event consumer panicked"));
            break;
        }
        // ChildHost and Peer roots retain exact tool/effect owners while joining this actor.
        // Their URI emitters may retain senders, but this actual terminal frame
        // has already been delivered and all actor-side settlement is finished.
        if child_shutdown {
            break;
        }
    }
    let pending = std::mem::take(&mut *io.pending.lock().unwrap());
    for (_, reply) in pending {
        let _ = reply.send(Err(anyhow::anyhow!("Native Session closed")));
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(()),
    }
}
struct NativeOutput {
    stop_after_child_shutdown: bool,
    pending:
        Arc<Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<Result<Value>>>>>,
    frames: NativeFrameSink,
    connection: CancellationToken,
    failure: Arc<Mutex<Option<String>>>,
    cancel_receipt: Arc<NativeCancelReceipt>,
}

impl Command {
    fn new(frame: WireValue) -> Self {
        let id = frame.get("id").cloned();
        let kind = frame
            .get("type")
            .and_then(WireValue::as_string)
            .and_then(|s| s.to_utf8().ok())
            .unwrap_or_default();
        Self { id, kind, frame }
    }

    fn string(&self, field: &str) -> Result<String> {
        let value = self
            .frame
            .get(field)
            .and_then(WireValue::as_string)
            .with_context(|| format!("{} requires string {field}", self.kind))?;
        value
            .to_utf8()
            .with_context(|| format!("{field} contains an unsupported unpaired UTF-16 surrogate"))
    }

    fn message(&self) -> Result<Message> {
        let text = self.string("message")?;
        let images = match self.frame.get("images") {
            None => Vec::new(),
            Some(WireValue::Array(images)) => images
                .iter()
                .map(|image| {
                    let image = Command::new(image.clone());
                    Ok(ImageContent {
                        data: image.string("data")?,
                        mime_type: image.string("mimeType")?,
                        detail: None,
                        compaction_frame: false,
                    })
                })
                .collect::<Result<Vec<_>>>()?,
            Some(_) => bail!("images must be an array"),
        };
        let mut message = UserMessage::text(text);
        if !images.is_empty() {
            let mut content = vec![UserBlock::text(message.content.plain_text())];
            content.extend(images.into_iter().map(UserBlock::Image));
            message.content = UserContent::Blocks(content);
        }
        Ok(Message::User(message))
    }

    fn queue_mode(&self) -> Result<QueueMode> {
        match self.string("mode")?.as_str() {
            "all" => Ok(QueueMode::All),
            "one-at-a-time" => Ok(QueueMode::OneAtATime),
            _ => bail!("mode must be all or one-at-a-time"),
        }
    }
}

struct Session {
    agent_context: Option<Arc<HostAgentContext>>,
    terminal_imports: Option<NativeTerminalImports>,
    edit_session: Mutex<Option<Arc<ara_cli::host_edit_session::HostEditSession>>>,
    values: Option<Arc<SessionValues>>,
    native_session: Option<Arc<ara_cli::native_factory::PreparedNativeSession>>,
    tool_owner: Mutex<super::CliToolOwner>,
    journal: tokio::sync::Mutex<SessionJournal>,
    projector: Arc<ara_cli::agent_message_projection::AgentMessageProjector>,
    artifacts: Arc<ara_cli::session_artifacts::SessionArtifacts>,
    // Public completed messages only. A partial is kept separately and never
    // appended by get_messages while the provider is still producing it.
    messages: Mutex<Vec<WireValue>>,
    partial: Mutex<Option<WireValue>>,
    persistence_error: Mutex<Option<String>>,
    failed_recovery_message: Mutex<Option<AgentMessage>>,
    name: Mutex<Option<String>>,
    header: Value,
    file: Option<std::path::PathBuf>,
}

impl Session {
    fn new(
        journal: Option<SessionJournal>,
        header: Value,
        messages: &[AgentMessage],
        values: Option<Arc<SessionValues>>,
        native_session: Option<Arc<ara_cli::native_factory::PreparedNativeSession>>,
    ) -> Result<Arc<Self>> {
        Self::new_with_agent_context(journal, header, messages, values, native_session, None)
    }

    fn new_with_agent_context(
        journal: Option<SessionJournal>,
        header: Value,
        messages: &[AgentMessage],
        values: Option<Arc<SessionValues>>,
        native_session: Option<Arc<ara_cli::native_factory::PreparedNativeSession>>,
        agent_context: Option<Arc<HostAgentContext>>,
    ) -> Result<Arc<Self>> {
        if let Some(agent) = &agent_context {
            anyhow::ensure!(header["id"].as_str() == Some(agent.auth_scope.primary_session_id.as_str()), "Agent context does not own actual Session identity");
        }
        validate_native_session_values(&values, &native_session)?;
        let mut journal = match journal {
            Some(journal) => journal,
            None => {
                let mut journal = SessionJournal::in_memory(header.clone())?;
                for message in messages {
                    journal.append_agent_message_body(&message.json()?)?;
                }
                journal
            }
        };
        let projector =
            Arc::new(ara_cli::agent_message_projection::AgentMessageProjector::default());
        if let Some(values) = &values {
            let encoder = Arc::new(ara_cli::actual_session::GraphJournalEncoder::new(
                values.clone(),
                None,
            )?);
            projector.bind_native_session(&mut journal, encoder)?;
        }
        // Native provenance comes from the active raw branch. Reopening never
        // needs the historical Skill file, nor equality against model content.
        let public_messages = Self::public_messages_with_projector(&journal, &projector)?;
        let artifacts = ara_cli::session_artifacts::SessionArtifacts::for_journal(&journal);
        let terminal_imports = native_session
            .as_deref()
            .map(NativeTerminalImports::prepare)
            .transpose()?;
        Ok(Arc::new(Self {
            terminal_imports,
            edit_session: Mutex::new(None),
            values,
            native_session,
            artifacts,
            agent_context,
            tool_owner: Mutex::new(super::CliToolOwner::default()),
            projector,
            name: Mutex::new(
                (!journal.title().title.is_empty()).then(|| journal.title().title.clone()),
            ),
            file: journal.is_persistent().then(|| journal.path().into()),
            journal: tokio::sync::Mutex::new(journal),
            messages: Mutex::new(public_messages),
            partial: Mutex::new(None),
            persistence_error: Mutex::new(None),
            failed_recovery_message: Mutex::new(None),
            header,
        }))
    }

    async fn bind_edit_session(&self, owner: &super::CliToolOwner) -> Result<()> {
        if let Some(read) = &owner.read_session {
            read.bind_session(
                self.header["id"]
                    .as_str()
                    .context("READ Session identity missing")?,
            )
            .map_err(|error| anyhow::Error::msg(error.0))?;
        }
        if let Some(edit) = &owner.edit_session {
            let journal = self.journal.lock().await;
            edit.bind(&journal, self.artifacts.clone())?;
            let mut current = self.edit_session.lock().unwrap();
            if current
                .as_ref()
                .is_some_and(|prior| !Arc::ptr_eq(prior, edit))
            {
                bail!("Session tools cannot replace their EDIT diagnostics owner");
            }
            *current = Some(edit.clone());
        }
        Ok(())
    }
    fn edit_session(&self) -> Option<Arc<ara_cli::host_edit_session::HostEditSession>> {
        self.edit_session.lock().unwrap().clone()
    }

    /// Reopen/statistics observe the settled CURRENT raw body through the same
    /// public redaction boundary as live MessageEnd. Typed source projections
    /// would silently discard application fields on known custom roles.
    fn public_entry_message(
        entry: &ara_session::Entry,
        projector: &ara_cli::agent_message_projection::AgentMessageProjector,
    ) -> Result<Option<WireValue>> {
        if let Some(body) = entry.agent_message_body()? {
            let message = projector.message_from_body(body, entry.original_source())?;
            return Ok(projector
                .public_event_wire(&AgentEvent::MessageEnd { message }, true)?
                .get("message")
                .cloned());
        }
        Ok(ara_cli::native_compaction::extension_event_message(entry)?.map(wire))
    }

    #[cfg(test)]
    fn public_messages(journal: &SessionJournal) -> Result<Vec<Value>> {
        Self::public_messages_with_projector(
            journal,
            &ara_cli::agent_message_projection::AgentMessageProjector::default(),
        )?
        .iter()
        .map(|body| ara_session::native_checked_serde(body).map_err(Into::into))
        .collect()
    }

    fn public_messages_with_projector(
        journal: &SessionJournal,
        projector: &ara_cli::agent_message_projection::AgentMessageProjector,
    ) -> Result<Vec<WireValue>> {
        let branch = journal.branch();
        let summary = journal
            .compacted_context_projection()
            .ok()
            .and_then(|projection| {
                projection.items.into_iter().find_map(|item| match item {
                    ara_session::CompactedContextItem::Summary(summary) => Some(summary),
                    _ => None,
                })
            });
        let start = summary
            .as_ref()
            .and_then(|summary| {
                branch
                    .iter()
                    .position(|entry| entry.id == summary.first_kept_entry_id)
            })
            .unwrap_or(0);
        let mut messages = Vec::new();
        if let Some(summary) = summary {
            let entry = branch
                .iter()
                .find(|entry| entry.id == summary.entry_id)
                .context("public summary source is outside the current branch")?;
            // Derived fixed summary DTO; never register it as canonical Agent input.
            let body = ara_session::context_view::compaction_summary_dto(
                entry,
                ara_session::context_view::SummaryViewPolicy::Provider,
                true,
                Some(&summary),
            )?;
            messages.push(body);
        }
        for entry in &branch[start..] {
            if let Some(message) = Self::public_entry_message(entry, projector)? {
                messages.push(message);
            }
        }
        Ok(messages)
    }
}

/// A durable Session publication cannot be followed by a failed public view
/// and then resumed through queued or side-request work.
fn public_messages_after_publication(
    journal: &SessionJournal,
    session: &Arc<Session>,
    connection: &CancellationToken,
    drain_queues: &mut bool,
) -> Result<Vec<WireValue>> {
    match Session::public_messages_with_projector(journal, &session.projector) {
        Ok(messages) => Ok(messages),
        Err(error) => {
            let observation =
                format!("Session public projection failed after publication: {error}");
            let mut primary = session.persistence_error.lock().unwrap();
            match primary.as_mut() {
                Some(primary) => {
                    primary.push_str("; secondary observation: ");
                    primary.push_str(&observation);
                }
                None => *primary = Some(observation),
            }
            drop(primary);
            connection.cancel();
            *drain_queues = false;
            Err(error.context("Session public projection failed after publication"))
        }
    }
}

struct RunSink {
    session: Arc<Session>,
    observed_usage: Option<Arc<ara_cli::auth_storage::AuthStorage>>,
    output: Output,
    cancel: CancellationToken,
    connection: CancellationToken,
    terminal: Mutex<Option<WireValue>>,
    messages: Mutex<Vec<WireValue>>,
    // Exact native IDs are assigned by the retained writer, not reconstructed
    // from public timestamps or the current branch's last entry.
    entries: Mutex<Vec<(String, Message)>>,
    header_guard: Mutex<GeminiHeaderGuard>,
}

fn edit_repair_usage_factory(
    session: Arc<Session>,
    account: Option<Arc<ara_cli::auth_storage::AuthStorage>>,
    connection: CancellationToken,
) -> super::host_edit_post_apply::UsageFactory {
    let session = Arc::downgrade(&session);
    Arc::new(move || {
        let session = session.clone();
        let account = account.clone();
        let connection = connection.clone();
        Box::pin(async move {
            let session = session
                .upgrade()
                .ok_or("Edit repair original Session closed")?;
            let owner = Arc::new(tokio::sync::Mutex::new(
                session.journal.lock().await.model_usage_owner(),
            ));
            let usage: super::host_edit_post_apply::UsageCallback = Arc::new(move |message| {
                let session = session.clone();
                let account = account.clone();
                let connection = connection.clone();
                let owner = owner.clone();
                Box::pin(async move {
                    let record = message.record();
                    let mut owner = owner.lock().await;
                    match session
                        .journal
                        .lock()
                        .await
                        .append_model_usage(&record, &owner)
                    {
                        Ok(Some(id)) => owner.parent_id = Some(id),
                        Ok(None) => {}
                        Err(error) => {
                            *session.persistence_error.lock().unwrap() = Some(error.to_string());
                            connection.cancel();
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
    })
}

fn skill_input(prompt: UserSkillPrompt) -> AgentMessage {
    ara_cli::agent_message_projection::raw_custom_input(prompt.event_message())
        .expect("validated Skill raw ingress")
}

fn input_skill(input: &AgentMessage) -> Result<Option<UserSkillPrompt>> {
    let public = input.json()?;
    if public["role"] != "custom"
        || public["customType"] != ara_session::SKILL_PROMPT_CUSTOM_TYPE
        || public["attribution"] != "user"
    {
        return Ok(None);
    }
    Ok((|| {
        Some(UserSkillPrompt {
            content: serde_json::from_value(public.get("content")?.clone()).ok()?,
            details: public.get("details").cloned(),
            timestamp: public.get("timestamp")?.as_i64()?,
        })
    })())
}

impl RunSink {
    fn completed_message(&self, message: WireValue) {
        self.messages.lock().unwrap().push(message.clone());
        self.session.messages.lock().unwrap().push(message);
        *self.session.partial.lock().unwrap() = None;
    }

    fn persistence_failed(&self, error: impl std::fmt::Display) {
        self.session
            .persistence_error
            .lock()
            .unwrap()
            .get_or_insert_with(|| error.to_string());
        self.cancel.cancel();
        self.connection.cancel();
    }
}

#[async_trait]
impl AgentEventSink for RunSink {
    async fn emit(&self, event: AgentEvent) {
        if self.session.persistence_error.lock().unwrap().is_some() {
            // Refused/failed tool ends remain observations of this cancelled
            // Run. They cannot append a message or authorize another effect.
            if matches!(
                &event,
                AgentEvent::ToolExecutionEnd { is_error: true, .. }
                    | AgentEvent::ActualToolExecutionEnd { is_error: true, .. }
            ) && let Ok(public) = self.session.projector.public_event_wire(&event, true)
            {
                self.output.frame_wire(public);
            }
            return;
        }
        if let AgentEvent::ActualMessageUpdate {
            message,
            event: assistant_event,
        } = &event
        {
            let headers = self
                .header_guard
                .lock()
                .unwrap()
                .observe_actual(assistant_event);
            if let Some(headers) = headers {
                let target = match self.session.values.as_deref() {
                    Some(values) => loop_guard::NativeHeaderTarget::capture(
                        message,
                        assistant_event.partial(),
                        values,
                    ),
                    None => Err(ara_agent::ActualProviderError::Protocol(
                        "Gemini header target has no Native Session values",
                    )),
                };
                let target = match target {
                    Ok(target) => target,
                    Err(error) => {
                        self.persistence_failed(error);
                        return;
                    }
                };
                let hit = self
                    .header_guard
                    .lock()
                    .unwrap()
                    .install_actual_hit(headers, target);
                if let Some(hit) = hit {
                    self.output.frame(json!({"type":"notice", "level":"warning", "source":"loop-guard",
                        "message":format!("Interrupted {} planning headers with no tool call; reminded the model to issue one.", hit.headers)}));
                    self.cancel.cancel();
                }
            }
        }
        if let AgentEvent::MessageUpdate {
            message,
            event: assistant_event,
        } = &event
        {
            let typed = match message.checked_model_snapshot() {
                Ok(typed) => typed,
                Err(error) => {
                    self.persistence_failed(error);
                    return;
                }
            };
            if let Some(Message::Assistant(message)) = typed {
                let hit = self
                    .header_guard
                    .lock()
                    .unwrap()
                    .observe(&message, assistant_event);
                if let Some(hit) = hit {
                    self.output.frame(json!({"type":"notice", "level":"warning", "source":"loop-guard",
                        "message":format!("Interrupted {} planning headers with no tool call; reminded the model to issue one.", hit.headers)}));
                    self.cancel.cancel();
                }
            }
        }
        let mut public = match self.session.projector.public_event_wire(&event, true) {
            Ok(public) => public,
            Err(error) => {
                self.persistence_failed(error);
                return;
            }
        };
        match &event {
            AgentEvent::MessageEnd { message } | AgentEvent::ActualMessageEnd { message, .. } => {
                let typed = if matches!(
                    message.body_snapshot(),
                    ara_agent::AgentMessageBody::Graph(_)
                ) {
                    None
                } else {
                    match message.checked_model_snapshot() {
                        Ok(typed) => typed,
                        Err(error) => {
                            self.persistence_failed(error);
                            return;
                        }
                    }
                };
                let mut journal = self.session.journal.lock().await;
                let native=self.session.tool_owner.lock().unwrap().native.clone();
                let peer_ingress=native.as_ref().map(|owner|owner.peer_ingress.clone());
                let wait_owner=native.as_ref().map(|owner|owner.wait_session.clone());
                let service=native.as_ref().and_then(|owner|owner.service_session.clone());
                let durable=peer_ingress.as_ref().is_some_and(|owner|owner.is_parent_claim(message))
                    ||wait_owner.as_ref().is_some_and(|owner|owner.requires_durable_append(message))
                    ||service.as_ref().is_some_and(|owner|owner.requires_durable_append(message));
                let appended=self.session.projector.append_current_body(&mut journal,message).and_then(|id|{if durable&&journal.is_persistent(){journal.materialize()?;}Ok(id)});
                match appended {
                    Ok(id) => {
                        #[cfg(feature="test-fixture")]
                        if durable {if let Err(error)=super::peer_host::fixture_frontier(if wait_owner.as_ref().is_some_and(|wait|wait.requires_durable_append(message)){"wait-after-materialize-before-ack"}else{"parent-after-materialize-before-ack"},journal.session_id()).await{self.persistence_failed(error);return;}}
                        if let Some(ingress)=&peer_ingress{ingress.append_parent_receipt(message,Ok(ara_cli::host_peers::ParentAdoption{session_id:journal.session_id().into(),entry_id:id.clone(),persistent:journal.is_persistent()&&journal.is_on_disk()}));}
                        let receipt=ara_cli::host_peers::ParentAdoption{session_id:journal.session_id().into(),entry_id:id.clone(),persistent:journal.is_persistent()&&journal.is_on_disk()};
                        if let Some(wait)=&wait_owner{if let Err(error)=wait.append_receipt(message,Ok(receipt.clone())){self.persistence_failed(error.0);}}
                        if let Some(service)=&service{service.append_receipt(message,Ok(ara_cli::host_services::CompletionAdoption{session_id:receipt.session_id,entry_id:receipt.entry_id,persistent:receipt.persistent}));}
                        if let Some(typed) = &typed {
                            self.entries.lock().unwrap().push((id, typed.clone()));
                        }
                    }
                    Err(error) => {
                        if let Some(ingress)=&peer_ingress{ingress.append_parent_receipt(message,Err(anyhow::anyhow!(error.to_string())));}
                        if let Some(wait)=&wait_owner{let _=wait.append_receipt(message,Err(error.to_string()));}
                        if let Some(service)=&service{service.append_receipt(message,Err(error.to_string()));}
                        self.persistence_failed(error);
                        drop(journal);
                        // Publish only the provider Assistant already received;
                        // failed input receipts stay unpublished. This does not
                        // register a Session/completion or durable entry receipt.
                        let assistant = match message.body_snapshot() {
                            ara_agent::AgentMessageBody::Graph(root) => {
                                self.session.values.as_ref().is_some_and(|values| {
                                    values.check_handle(&root).and_then(|()| {
                                        let role = values.heap().get(&root, &"role".into(), values.runtime())?;
                                        values.heap().snapshot(&role)
                                    }).is_ok_and(|role| matches!(role,
                                        ara_rpc::value_heap::ValueSnapshot::String(role) if role.equals_ascii("assistant")))
                                })
                            }
                            _ => matches!(typed, Some(Message::Assistant(_))),
                        };
                        if assistant {
                            self.output.frame_wire(public);
                        }
                        return;
                    }
                }
                drop(journal);
                if let Some(owner) = &self.observed_usage {
                    if matches!(
                        message.body_snapshot(),
                        ara_agent::AgentMessageBody::Graph(_)
                    ) {
                        match self.session.projector.observed_actual_usage(message) {
                            Ok(Some(entry)) => owner.record_observed_usage(&[entry], None),
                            Ok(None) => {}
                            Err(error) => {
                                self.persistence_failed(error);
                                return;
                            }
                        }
                    } else if let Some(Message::Assistant(message)) = &typed {
                        owner.record_assistant_usage(message);
                    }
                }
                self.completed_message(public.get("message").cloned().unwrap_or(WireValue::Null));
            }
            AgentEvent::MessageStart { .. }
                if public
                    .get("message")
                    .is_some_and(|message| public_wire_is(message, "role", "assistant")) =>
            {
                *self.session.partial.lock().unwrap() =
                    Some(public.get("message").cloned().unwrap_or(WireValue::Null));
            }
            AgentEvent::MessageUpdate { .. } | AgentEvent::ActualMessageUpdate { .. } => {
                *self.session.partial.lock().unwrap() =
                    Some(public.get("message").cloned().unwrap_or(WireValue::Null));
            }
            AgentEvent::AgentEnd { .. } | AgentEvent::ActualAgentEnd { .. } => {
                *self.session.partial.lock().unwrap() = None;
                public.insert(
                    "messages",
                    WireValue::Array(self.messages.lock().unwrap().clone()),
                );
                *self.terminal.lock().unwrap() = Some(public);
                return;
            }
            _ => {}
        }
        self.output.frame_wire(public);
    }
}

// Select the original finalized record in report order. Graph messages must
// never cross the legacy checked-model boundary to reach account feedback.
fn completed_assistant(
    report: &RunReport,
    projector: &ara_cli::agent_message_projection::AgentMessageProjector,
) -> Result<(Option<AssistantMessage>, Option<AgentMessage>)> {
    for raw in report.messages.iter().rev() {
        if let ara_agent::AgentMessageBody::Graph(root) = raw.body_snapshot() {
            let values = projector
                .actual_values()
                .context("actual terminal selection has no Session values")?;
            values.check_handle(&root)?;
            let role = values.heap().get(&root, &"role".into(), values.runtime())?;
            if matches!(values.heap().snapshot(&role)?,
                ara_rpc::value_heap::ValueSnapshot::String(role) if role.equals_ascii("assistant"))
            {
                return Ok((None, Some(raw.clone())));
            }
        } else if let Some(Message::Assistant(message)) = raw.checked_model_snapshot()? {
            return Ok((Some(message), None));
        }
    }
    Ok((None, None))
}

// An explicit final display boundary. No display field grants retry/reset or
// substitutes for the canonical record used by the recovery consumers.
fn actual_terminal_error(
    message: &AgentMessage,
    projector: &ara_cli::agent_message_projection::AgentMessageProjector,
) -> Result<Option<String>> {
    let ara_agent::AgentMessageBody::Graph(root) = message.body_snapshot() else {
        bail!("actual terminal display received a legacy record")
    };
    let values = projector
        .actual_values()
        .context("actual terminal display has no Session values")?;
    values.check_handle(&root)?;
    let error = values
        .heap()
        .get(&root, &"errorMessage".into(), values.runtime())?;
    if matches!(
        values.heap().snapshot(&error)?,
        ara_rpc::value_heap::ValueSnapshot::Undefined | ara_rpc::value_heap::ValueSnapshot::Null
    ) {
        return Ok(None);
    }
    let text = values.heap().to_string(&error, values.runtime())?;
    Ok(Some(text.to_utf8().context(
        "actual terminal error is not Unicode scalar display text",
    )?))
}

struct ActiveRun {
    child_turn:Option<Arc<super::child_host::ChildTurn>>,
    native: Option<Arc<ara_cli::native_run::NativeRunLease>>,
    cancel: CancellationToken,
    task: JoinHandle<std::result::Result<Option<RunReport>, AgentError>>,
    command: Command,
    sink: Arc<RunSink>,
}

struct PendingRetryError {
    entry_id: String,
    message: RecoveryTerminal,
    attempt: usize,
    recovery: String,
    note: String,
}

struct RetrySaga {
    session: Arc<Session>,
    generation: u64,
    command: Command,
    attempt: usize,
    pending: Vec<PendingRetryError>,
    native_mode: bool,
    native_context: Option<NativeRetryContextReceipt>,
    native_guard: Option<NativeRetryWaitGuard>,
    deadline: Option<Instant>,
    cancel: CancellationToken,
    visible: bool,
    expected_messages: Vec<Value>,
}

struct PendingHeaderContinue {
    session: Arc<Session>,
    generation: u64,
    command: Command,
    expected_messages: Vec<Value>,
    deadline: Instant,
}

#[derive(Clone)]
enum BashDestination {
    Current,
    Detached { parent: Option<String> },
    Branch { parent: Option<String> },
}

// Host-owned destinations are shared by jobs started in one ownership scope.
// Completions and transitions mutate them only in the serial command owner.
struct BashTarget {
    session: Arc<Session>,
    destination: BashDestination,
}

struct BashJob {
    command: Command,
    text: String,
    target: Arc<Mutex<BashTarget>>,
    cancel: CancellationToken,
    task: JoinHandle<()>,
}

type BashCompletion = (
    u64,
    std::result::Result<
        std::result::Result<ara_tools::bash::BashResult, ara_agent::ToolError>,
        tokio::task::JoinError,
    >,
);

struct BashDispatcher {
    current: Mutex<Arc<Mutex<BashTarget>>>,
    jobs: Mutex<std::collections::HashMap<u64, BashJob>>,
    next_id: std::sync::atomic::AtomicU64,
    done: mpsc::UnboundedSender<BashCompletion>,
    output: Output,
    connection: CancellationToken,
    cwd: PathBuf,
}

impl BashDispatcher {
    // Fixed RpcInputDispatcher starts bash as soon as stdin dispatches it,
    // independently of the ordinary serial command tail. The process task
    // cannot write a journal: the Host owns every completion and transition.
    fn dispatch(&self, command: Command) {
        let text = match command.string("command") {
            Ok(text) => text,
            Err(error) => {
                self.output
                    .response(&command, None, Some(error.to_string()));
                return;
            }
        };
        let target = self.current.lock().unwrap().clone();
        let cancel = self.connection.child_token();
        let process_cancel = cancel.clone();
        let context = ara_tools::ToolContext::new(self.cwd.clone());
        let args = json!({"command":text}).as_object().unwrap().clone();
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let done = self.done.clone();
        // Insert and install the supervisor under one lock. A fast completion
        // therefore cannot outrun its ownership record.
        let mut jobs = self.jobs.lock().unwrap();
        let process = tokio::spawn(async move {
            ara_tools::bash::execute_bash(&context, &args, process_cancel, Arc::new(|_| {})).await
        });
        let task = tokio::spawn(async move {
            let result = process.await;
            let _ = done.send((id, result));
        });
        jobs.insert(
            id,
            BashJob {
                command,
                text,
                target,
                cancel,
                task,
            },
        );
    }

    fn abort(&self) {
        for job in self.jobs.lock().unwrap().values() {
            job.cancel.cancel();
        }
    }

    fn is_empty(&self) -> bool {
        self.jobs.lock().unwrap().is_empty()
    }
}

struct PendingBash {
    target: Arc<Mutex<BashTarget>>,
    message: BashExecutionMessage,
}

// Fixed B-651c45def5 observation only. None preserves normal Host behavior.
#[cfg(test)]
struct FixedSnapcompactObservation {
    usage_tokens: usize,
    frame_rescue_result: Option<ara_cli::snapcompact::PreparedSnapcompact>,
    frame_rescue_returns: usize,
    shake_calls: Vec<(bool, ara_agent::compaction::local_reduction::ShakeConfig)>,
}

struct PreparedPeerActivation {
    attachment:super::peer_host::PreparedPeerAttachment,
    root:String,
    persistent:bool,
    uri_peer:Arc<dyn ara_tools::ContentUriPort>,
    router:Arc<ara_cli::session_artifacts::ArtifactUriRouter>,
}
// One controlled foreign-realm catalog at the actual publisher boundary.
// The normal build has no fault state or alternate publication behavior.
#[cfg(feature="test-fixture")]
struct ColdPublicationFault {
    _donor: Arc<ara_cli::native_factory::PreparedNativeSession>,
    snapshot: Arc<ara_cli::native_factory::catalog::NativeCatalogSnapshot>,
    owner_before: Value,
    prepared_primary: Option<Value>,
    publication_attempted: bool,
    native_error: Option<String>,
}

struct Host {
    #[cfg(feature="test-fixture")]
    cold_publication_fault: Mutex<Option<ColdPublicationFault>>,
    task_dispatch:Option<super::task_host::TaskDispatch>,
    peer_runtime:Option<ara_cli::host_peers::RuntimeKey>,
    peer_sender:Option<mpsc::UnboundedSender<super::peer_host::PeerActorRequest>>,
    peer_wake:Option<super::peer_host::PendingWake>,
    peer_deferred:std::collections::VecDeque<super::peer_host::PendingWake>,
    child_turn:Option<Arc<super::child_host::ChildTurn>>,
    agent: Arc<Agent>,
    session: Arc<Session>,
    config: AgentConfig,
    skills: Vec<LoadedSkill>,
    max_time: Option<f64>,
    output: Output,
    connection: CancellationToken,
    active: Option<ActiveRun>,
    handoff_control: Arc<Mutex<handoff::HandoffControl>>,
    quota_control: Arc<Mutex<quota::QuotaControl>>,
    model_selection_control: Arc<Mutex<quota::QuotaControl>>,
    maintenance: Option<maintenance::ActiveMaintenance>,
    maintenance_continue: Option<maintenance::PendingMaintenanceContinue>,
    terminal_recovery: maintenance::TerminalRecoveryState,
    local_context_rebase: Option<reduction::ContextRebase>,
    #[cfg(test)]
    fixed_snapcompact_observation: Option<Arc<Mutex<FixedSnapcompactObservation>>>,
    sessions: SessionFactory,
    tool_bridge: Arc<ToolBridge>,
    uri_bridge: Arc<UriBridge>,
    artifact_router: Arc<ara_cli::session_artifacts::ArtifactUriRouter>,
    host_tools: Vec<HostToolDefinition>,
    snapshot: Arc<RwLock<ExecutionSnapshot>>,
    snapshot_publication: Arc<ara_cli::append_only_context_mode::SnapshotPublication>,
    admission_tx: mpsc::UnboundedSender<fallback::AdmissionRequest>,
    route_generation: Arc<std::sync::atomic::AtomicU64>,
    // Deliberate abort keeps queued input visible but suppresses autonomous
    // resumption. A new explicit prompt/queue command permits another Run.
    drain_queues: bool,
    bash_target: Arc<Mutex<BashTarget>>,
    bash_targets: Vec<Weak<Mutex<BashTarget>>>,
    bash_dispatcher: Arc<BashDispatcher>,
    pending_bash: Vec<PendingBash>,
    bash_error: Option<String>,
    compaction_policy: AutoCompactionPolicy,
    is_compacting: bool,
    auto_compaction_pending: bool,
    // A failed/no-progress pass is not billed again for the same source view.
    auto_compaction_checked: Option<(String, Option<String>, usize)>,
    retry_policy: RetryPolicy,
    retry: Option<RetrySaga>,
    fallback: fallback::FallbackState,
    auto: auto::AutoState,
    model_usage_order: Arc<ara_cli::model_usage_order::SqliteModelUsageOrder>,
    model_cycle_scope: Vec<ara_cli::model_scope::SessionScopedModel>,
    prompt_generation: u64,
    loop_guard_settings: LoopGuardSettings,
    unexpected_stop_mode: UnexpectedStopMode,
    header_continue: Option<PendingHeaderContinue>,
    input_closed: bool,
    tool_loop: Arc<Mutex<ToolLoopState>>,
    tool_loop_settings_dir: Option<PathBuf>,
}

fn same_session_identity(left: &Session, header: &Value, file: Option<&std::path::Path>) -> bool {
    if left.header["id"] != header["id"] {
        return false;
    }
    match (left.file.as_deref(), file) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            std::fs::canonicalize(left)
                .ok()
                .zip(std::fs::canonicalize(right).ok())
                .is_some_and(|(a, b)| a == b)
                || lexical_absolute(left)
                    .ok()
                    .zip(lexical_absolute(right).ok())
                    .is_some_and(|(a, b)| a == b)
        }
        _ => false,
    }
}

fn same_session_file(left: &Session, right: &Session) -> bool {
    same_session_identity(left, &right.header, right.file.as_deref())
}

fn same_session_values(
    left: &Option<Arc<SessionValues>>,
    right: &Option<Arc<SessionValues>>,
) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => Arc::ptr_eq(left, right),
        _ => false,
    }
}

fn validate_native_session_values(
    values: &Option<Arc<SessionValues>>,
    owner: &Option<Arc<ara_cli::native_factory::PreparedNativeSession>>,
) -> Result<()> {
    if let Some(owner) = owner {
        owner.check_values(
            values
                .as_ref()
                .context("Native Session owner has no selected values")?,
        )?;
    }
    Ok(())
}

fn same_native_session(
    left: &Option<Arc<ara_cli::native_factory::PreparedNativeSession>>,
    right: &Option<Arc<ara_cli::native_factory::PreparedNativeSession>>,
) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => Arc::ptr_eq(left, right),
        _ => false,
    }
}

fn validate_native_session_hooks(
    config: &AgentConfig,
    values: &Option<Arc<SessionValues>>,
    owner: &Option<Arc<ara_cli::native_factory::PreparedNativeSession>>,
) -> Result<()> {
    validate_native_session_values(values, owner)?;
    if let Some(owner) = owner {
        let values = values
            .as_ref()
            .context("Native Session owner has no selected values")?;
        let expected = owner.require_tool_hooks()?.source(values)?;
        let selected = config
            .hooks
            .actual_tool_source(values)?
            .context("Native Session hooks have no original builtin source")?;
        if !Arc::ptr_eq(&expected, &selected) {
            bail!("Native Session hooks do not retain the prepared original source");
        }
    }
    Ok(())
}

fn validate_session_values(
    config: &AgentConfig,
    controller: Option<&ara_cli::append_only_context_mode::SessionContextController>,
    selected: &Option<Arc<SessionValues>>,
) -> Result<()> {
    let captured = config
        .hooks
        .provider_context_binding()
        .and_then(|binding| binding.actual_values);
    if !same_session_values(selected, &captured) {
        bail!("Session values do not match the candidate hook binding");
    }
    if let Some(controller) = controller
        && !same_session_values(selected, &controller.binding().actual_values)
    {
        bail!("Session values do not match the candidate controller binding");
    }
    Ok(())
}

fn bash_result_value(result: &ara_tools::bash::BashResult) -> Value {
    let mut value = json!({"output":result.output_with_status_notice(),"cancelled":result.cancelled,"truncated":result.truncated,
        "totalLines":result.total_lines,"totalBytes":result.total_bytes,
        "outputLines":result.output_lines,"outputBytes":result.output_bytes});
    if let Some(exit) = result.exit_code {
        value["exitCode"] = json!(exit);
    }
    if result.timed_out {
        value["timedOut"] = json!(true);
    }
    if let Some(cwd) = &result.working_dir {
        value["workingDir"] = json!(cwd);
    }
    value
}

impl Host {
    async fn task_request(&mut self,request:super::task_host::TaskEnvelope){
        let prepared=async{
            anyhow::ensure!(self.session.header["id"].as_str()==Some(request.session_id.as_str()),"Task belongs to an old logical Session; no replacement/replay");
            let native=self.sessions.tool_owner.native.as_ref().context("Task Native owner disappeared")?;
            let policy=native.task_session.policy();
            let journal=self.session.journal.lock().await;
            super::task_host::prepare(&self.sessions,&journal,request.request,policy,&request.cancel).await
        }.await;
        match prepared{
            Ok(tasks)=>match(&self.sessions.peers,&self.task_dispatch){
                (Some(peers),Some(dispatch))=>super::task_host::launch(tasks,peers.clone(),dispatch.clone(),request.cancel,request.reply),
                _=>{let _=request.reply.send(Err(anyhow::anyhow!("Task Host execution owner closed")));}
            },
            Err(error)=>{let _=request.reply.send(Err(error));}
        }
    }

    async fn finish_retry(
        &mut self,
        success: Option<&AssistantMessage>,
        error: Option<String>,
        supersede: bool,
    ) {
        let success = success.map(|message| RecoveryTerminal::Legacy(message.clone()));
        Box::pin(self.finish_retry_terminal(success.as_ref(), error, supersede)).await;
    }

    async fn finish_retry_terminal(
        &mut self,
        success: Option<&RecoveryTerminal>,
        error: Option<String>,
        supersede: bool,
    ) {
        let Some(saga) = self.retry.take() else {
            return;
        };
        if saga.native_mode {
            Box::pin(self.finish_retry_native(saga, success, error, supersede)).await;
            return;
        }
        let success = success.and_then(RecoveryTerminal::legacy);
        saga.cancel.cancel();
        let mut final_error = error;
        let mut updates = Vec::new();
        let messages = self.agent.raw_messages().await;
        let checked = messages
            .iter()
            .map(|raw| raw.checked_model_snapshot())
            .collect::<std::result::Result<Vec<_>, _>>();
        let failed_public = saga
            .pending
            .iter()
            .map(|pending| {
                AgentEvent::MessageEnd {
                    message: Message::Assistant(
                        pending
                            .message
                            .legacy()
                            .expect("legacy saga owns legacy failures")
                            .clone(),
                    )
                    .into(),
                }
                .full()
                .map(|value| wire(value["message"].clone()))
            })
            .collect::<std::result::Result<Vec<_>, _>>();
        let projection = checked.and_then(|checked| failed_public.map(|public| (checked, public)));
        if let Err(error) = &projection {
            final_error.get_or_insert_with(|| error.to_string());
            saga.session
                .persistence_error
                .lock()
                .unwrap()
                .get_or_insert_with(|| error.to_string());
            self.connection.cancel();
        }
        if let Ok((checked, failed_public)) = projection
            && (success.is_some() || supersede)
        {
            let records = saga.pending.iter().map(|pending| {
                let recovery = AssistantRetryRecovery {
                    kind: "auto-retry".into(),
                    status: if success.is_some() { "recovered" } else { "superseded" }.into(),
                    attempt: pending.attempt,
                    recovery: pending.recovery.clone(),
                    note: pending.note.clone(),
                    recovered_at: success.map(|_| chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
                    superseded_by: success.map(|message| {
                        let mut value = json!({"timestamp":message.timestamp,"provider":message.provider,"model":message.model});
                        if let Some(id) = &message.response_id { value["responseId"] = json!(id); }
                        value
                    }),
                };
                (pending.entry_id.clone(), recovery)
            }).collect::<Vec<_>>();
            let persisted = saga
                .session
                .journal
                .lock()
                .await
                .update_retry_recovery(&records);
            match persisted {
                Ok(()) => {
                    let mut public = saga.session.messages.lock().unwrap();
                    'recoveries: for ((pending, (entry_id, recovery)), failed) in
                        saga.pending.iter().zip(&records).zip(&failed_public)
                    {
                        for (raw, checked) in messages.iter().zip(&checked) {
                            if checked.as_ref().and_then(Message::as_assistant)
                                == pending.message.legacy()
                            {
                                let body = match raw.body_snapshot() {
                                    ara_agent::AgentMessageBody::Model(Message::Assistant(
                                        mut current,
                                    )) => {
                                        current.retry_recovery = Some(recovery.clone());
                                        ara_agent::AgentMessageBody::Model(Message::Assistant(
                                            current,
                                        ))
                                    }
                                    ara_agent::AgentMessageBody::RawJson(mut body) => {
                                        body["retryRecovery"] = json!(recovery);
                                        ara_agent::AgentMessageBody::RawJson(body)
                                    }
                                    _ => continue,
                                };
                                if let Err(error) = raw.replace_body(body) {
                                    let reason =
                                        format!("Retry recovery projection failed: {error}");
                                    final_error.get_or_insert_with(|| reason.clone());
                                    saga.session
                                        .persistence_error
                                        .lock()
                                        .unwrap()
                                        .get_or_insert(reason);
                                    self.connection.cancel();
                                    break 'recoveries;
                                }
                                raw.invalidate_derived();
                            }
                        }
                        for message in public
                            .iter_mut()
                            .filter(|message| message.deep_equal(failed))
                        {
                            message.insert("retryRecovery", wire(json!(recovery)));
                        }
                        updates.push(json!({"entryId":entry_id,"persistenceKey":super::rpc_host_retry::persistence_key(pending.message.legacy().expect("legacy saga owns legacy failures")),
                            "note":recovery.note,"retryRecovery":recovery}));
                    }
                    drop(public);
                    if let Err(error) = self.agent.replace_idle_raw_messages(messages) {
                        final_error = Some(format!("Retry recovery projection failed: {error}"));
                        *saga.session.persistence_error.lock().unwrap() = final_error.clone();
                        self.connection.cancel();
                    }
                }
                Err(error) => {
                    final_error = Some(format!("Retry recovery persistence failed: {error}"));
                    *saga.session.persistence_error.lock().unwrap() = final_error.clone();
                    self.connection.cancel();
                }
            }
        }
        let mut event = json!({"type":"auto_retry_end","success":success.is_some() && final_error.is_none(),"attempt":saga.attempt});
        if let Some(error) = final_error {
            event["finalError"] = json!(error);
        }
        if success.is_some() || supersede {
            event["retryErrors"] = json!(updates);
        }
        self.output.frame(event);
    }

    async fn abort_retry(&mut self) {
        if self
            .retry
            .as_ref()
            .is_some_and(|retry| retry.deadline.is_some())
        {
            self.finish_retry(None, Some("Retry cancelled".into()), false)
                .await;
            self.drain_queues = false;
        } else if let Some(retry) = &mut self.retry {
            // Fixed abortRetry resolves its wait promise; an already scheduled
            // Agent continuation is owned by the Run and is not aborted here.
            retry.visible = false;
        }
    }

    async fn begin_retry(
        &mut self,
        active: &ActiveRun,
        message: &AssistantMessage,
        quota: Option<&super::session_quota_recovery::SessionQuotaOutcome>,
    ) -> Result<bool> {
        self.begin_retry_terminal(active, &RecoveryTerminal::Legacy(message.clone()), quota)
            .await
    }

    async fn begin_retry_terminal(
        &mut self,
        active: &ActiveRun,
        message: &RecoveryTerminal,
        quota: Option<&super::session_quota_recovery::SessionQuotaOutcome>,
    ) -> Result<bool> {
        use super::rpc_host_retry::{RetryDisposition, backoff_ms};
        message.check_host(self)?;
        let retry_model = self.config.model.clone();
        let Some(mut class) = message.retry_class(&retry_model)? else {
            return Ok(false);
        };
        let usage_admission = message.usage_admission()?;
        let recorded_quota =
            quota.filter(|outcome| outcome.recorded && class.usage_limit && usage_admission);
        if recorded_quota.is_some() {
            class.replay_blocked = false;
        }
        if active.cancel.is_cancelled()
            || self.connection.is_cancelled()
            || !Arc::ptr_eq(&active.sink.session, &self.session)
            || !self.retry_policy.enabled()
            || quota.is_some_and(|outcome| !outcome.recorded)
            || quota.is_some() && self.quota_control.lock().unwrap().stop_requested()
        {
            return Ok(false);
        }
        let mut messages = self.agent.raw_messages().await;
        let Some((same_route_eligible, disposition)) =
            message.retry_dispositions(&messages, &class)?
        else {
            return Ok(false);
        };
        let max_retries = message.max_retries(self.retry_policy.max_retries())?;
        let entry_id = message.persisted_id(active)?;
        let admitted_context = messages.clone();
        if message.actual().is_some() {
            let journal = self.session.journal.lock().await;
            message.source(&journal, &entry_id)?;
            message.check_host(self)?;
        }
        let previous_attempt = self.retry.as_ref().map_or(0, |retry| retry.attempt);
        let mut attempt = previous_attempt.saturating_add(1);
        let switched_credential = recorded_quota.is_some_and(|outcome| outcome.switched);
        let retry_now_ms = chrono::Utc::now().timestamp_millis() as f64;
        let parsed_retry_after_ms = message.retry_hint(retry_now_ms)?;
        // Quota-owned waits remain authoritative. Do not re-inject the
        // merged provider wait after Native formatter/grammar suppression.
        let wait_ms = recorded_quota
            .and_then(|outcome| outcome.wait_ms)
            .or(parsed_retry_after_ms);
        // Fixed turn-recovery.ts:2264-2267 requires both a sibling fact and
        // its effective bounded wait, including the final exhausted attempt.
        let wait_for_sibling = recorded_quota
            .is_some_and(|outcome| outcome.sibling_retry_at_ms.is_some())
            && wait_ms.is_some_and(|wait| wait <= self.retry_policy.max_delay_ms());
        let switched_model = if self.retry_policy.model_fallback()
            && !class.stale_responses
            && !class.abort
            && !switched_credential
            && !wait_for_sibling
            && class.error_id
                & (ara_ai::retry_classification::flag::THINKING_LOOP
                    | ara_ai::retry_classification::flag::CONTENT_BLOCKED)
                == 0
        {
            self.try_retry_model_fallback_terminal(
                active,
                message,
                disposition,
                parsed_retry_after_ms,
                retry_now_ms,
            )
            .await?
        } else {
            false
        };
        if active.cancel.is_cancelled()
            || self.connection.is_cancelled()
            || self.quota_control.lock().unwrap().stop_requested()
        {
            return Ok(false);
        }
        if message.actual().is_some() {
            message.check_host(self)?;
            let Some(mut current_class) = message.retry_class(&retry_model)? else {
                return Ok(false);
            };
            if recorded_quota.is_some() && current_class.usage_limit && message.usage_admission()? {
                current_class.replay_blocked = false;
            }
            let Some((_, current_disposition)) =
                message.retry_dispositions(&messages, &current_class)?
            else {
                return Ok(false);
            };
            if current_disposition != disposition {
                bail!("Native retry disposition changed while preparing fallback")
            }
            class = current_class;
        }
        // Native spends ordinary hops cumulatively. Only an actual model hop
        // after exhaustion starts a fresh model budget; credential rotation
        // retains the cumulative count while trying distinct accounts.
        if attempt as f64 > max_retries && switched_model {
            attempt = 1;
        }
        if attempt as f64 > max_retries && !switched_model && !switched_credential {
            // The terminal failure remains raw; only prior retry receipts are
            // marked superseded, as fixed turn-recovery.ts:2310-2325.
            if self.retry.is_none() {
                self.retry = Some(RetrySaga {
                    session: self.session.clone(),
                    generation: self.prompt_generation,
                    command: Command {
                        id: active.command.id.clone(),
                        kind: active.command.kind.clone(),
                        frame: active.command.frame.clone(),
                    },
                    attempt: previous_attempt,
                    pending: Vec::new(),
                    native_mode: message.actual().is_some(),
                    native_context: message.context_receipt(&messages)?,
                    native_guard: None,
                    deadline: None,
                    cancel: self.connection.child_token(),
                    visible: true,
                    expected_messages: if message.actual().is_some() {
                        Vec::new()
                    } else {
                        self.session.projector.current_body_receipts(&messages)?
                    },
                });
            }
            self.finish_retry(None, message.error_message()?, true)
                .await;
            return Ok(false);
        }
        if !same_route_eligible && !switched_model {
            self.finish_retry(None, message.error_message()?, true)
                .await;
            return Ok(false);
        }
        let randomness = uuid::Uuid::new_v4();
        let random = u64::from_le_bytes(randomness.as_bytes()[..8].try_into().unwrap()) as f64
            / u64::MAX as f64;
        let mut delay_ms = if class.stale_responses {
            0.0
        } else {
            backoff_ms(self.retry_policy.base_delay_ms(), attempt, random)
        };
        // CURRENT rechecks compare the provider hint to its original baseline,
        // with the ordinary backoff floor. Quota may authorize an earlier
        // sibling wait without changing that provider hint.
        let provider_wait_ceiling = parsed_retry_after_ms.map_or(delay_ms, |wait| wait.max(delay_ms));
        // Fixed turn-recovery.ts:2171-2174,2303-2306: a same-route hint
        // may raise the backoff but must never shorten it. Stale Responses
        // recovery keeps its independent zero-delay branch.
        if switched_model || switched_credential {
            delay_ms = 0.0;
        } else if !class.stale_responses
            && let Some(wait_ms) = wait_ms
            && wait_ms > delay_ms
        {
            delay_ms = wait_ms;
        }
        if !delay_ms.is_finite()
            || delay_ms < 0.0
            || (self.retry_policy.max_delay_ms() > 0.0
                && delay_ms > self.retry_policy.max_delay_ms())
        {
            if let Some(retry) = &mut self.retry {
                retry.attempt = attempt;
            } else {
                self.retry = Some(RetrySaga {
                    session: self.session.clone(),
                    generation: self.prompt_generation,
                    command: Command {
                        id: active.command.id.clone(),
                        kind: active.command.kind.clone(),
                        frame: active.command.frame.clone(),
                    },
                    attempt,
                    pending: Vec::new(),
                    native_mode: message.actual().is_some(),
                    native_context: message.context_receipt(&messages)?,
                    native_guard: None,
                    deadline: None,
                    cancel: self.connection.child_token(),
                    visible: true,
                    expected_messages: Vec::new(),
                });
            }
            self.finish_retry(
                None,
                Some(format!(
                    "Provider requested {}ms wait, exceeds retry.maxDelayMs ({}ms). Original error: {}",
                    delay_ms.ceil(),
                    self.retry_policy.max_delay_ms(),
                    message.error_message()?.as_deref().unwrap_or("Unknown error")
                )),
                false,
            )
            .await;
            return Ok(false);
        }
        let duration = Duration::try_from_secs_f64(delay_ms / 1000.0)
            .context("retry delay is not representable")?;
        // Fixed scheduleAgentContinue defers by one millisecond after backoff;
        // a zero wait still gives the serial reader a command boundary.
        let deadline = Instant::now()
            .checked_add(duration.saturating_add(Duration::from_millis(1)))
            .context("retry deadline is not representable")?;
        self.agent.replace_idle_raw_messages(messages.clone())?;
        let native_context = message.context_receipt(&messages)?;
        let saga = self.retry.get_or_insert_with(|| RetrySaga {
            session: self.session.clone(),
            generation: self.prompt_generation,
            command: Command {
                id: active.command.id.clone(),
                kind: active.command.kind.clone(),
                frame: active.command.frame.clone(),
            },
            attempt: 0,
            pending: Vec::new(),
            native_mode: message.actual().is_some(),
            native_context,
            native_guard: None,
            deadline: None,
            cancel: self.connection.child_token(),
            visible: true,
            expected_messages: Vec::new(),
        });
        if !Arc::ptr_eq(&saga.session, &self.session) || saga.generation != self.prompt_generation {
            bail!("retry owner no longer matches this Session generation");
        }
        let recovery = if switched_model {
            "model"
        } else if switched_credential {
            "credential"
        } else if class.usage_limit && delay_ms > 0.0 {
            "wait"
        } else {
            "plain"
        };
        let note = if switched_model {
            "error; switched model; retried"
        } else if class.usage_limit {
            if switched_credential {
                "rate-limited; switched account; retried"
            } else if recovery == "wait" {
                "rate-limited; waited; retried"
            } else {
                "rate-limited; retried"
            }
        } else {
            "error; retried"
        };
        if !saga
            .pending
            .iter()
            .any(|pending| pending.entry_id == entry_id)
        {
            saga.pending.push(PendingRetryError {
                entry_id,
                message: message.clone(),
                attempt,
                recovery: recovery.into(),
                note: note.into(),
            });
        }
        saga.attempt = attempt;
        saga.visible = true;
        saga.deadline = Some(deadline);
        self.output.frame(json!({"type":"auto_retry_start","attempt":attempt,"maxAttempts":max_retries,
            "delayMs":delay_ms,"errorMessage":message.error_message()?.as_deref().unwrap_or("Unknown error"),"errorId":class.error_id}));
        if disposition == RetryDisposition::RemoveFailed {
            if !message.exact_tail(&messages)? {
                bail!("failed retry assistant is no longer the exact active tail");
            }
            messages.pop();
            self.agent.replace_idle_raw_messages(messages.clone())?;
            let failed = message.public_message(&self.session)?;
            let mut public = self.session.messages.lock().unwrap();
            if let Some(index) = public
                .iter()
                .rposition(|candidate| candidate.deep_equal(&failed))
            {
                public.remove(index);
            }
        }
        if class.error_id & ara_ai::retry_classification::flag::THINKING_LOOP != 0
            && loop_guard::guard_enabled(self.loop_guard_settings)
        {
            let notice = LoopGuardNotice::thinking_loop();
            let id = self
                .session
                .journal
                .lock()
                .await
                .append_loop_guard_notice(&notice)?;
            let raw = self
                .session
                .projector
                .new_custom_input(notice.event_message())?;
            self.session.projector.record_entry_id(&raw, id);
            messages.push(raw);
            self.agent.replace_idle_raw_messages(messages.clone())?;
            let public = notice.event_message();
            self.session
                .messages
                .lock()
                .unwrap()
                .push(wire(public.clone()));
            self.output
                .frame(json!({"type":"message_start","message":public}));
            self.output
                .frame(json!({"type":"message_end","message":public}));
        }
        saga.expected_messages = if saga.native_mode {
            Vec::new()
        } else {
            self.session.projector.current_body_receipts(&messages)?
        };
        saga.native_context = message.context_receipt(&messages)?;
        saga.native_guard = message.actual().map(|_| NativeRetryWaitGuard {
            terminal: message.clone(),
            context: admitted_context,
            disposition,
            usage_admitted: recorded_quota.is_some(),
            switched: switched_model || switched_credential,
            attempt,
            wait_ceiling: provider_wait_ceiling,
        });
        if class.stale_responses {
            self.config.provider = self.sessions.provider.build();
            if let Some(controller) = &self.sessions.context_controller {
                controller.fresh_provider_session();
            }
            self.publish_snapshot(&self.config);
        }
        self.auto_compaction_pending = false;
        Ok(true)
    }

    async fn resume_retry(&mut self) {
        let Some(saga) = &self.retry else { return };
        if saga.cancel.is_cancelled()
            || saga.generation != self.prompt_generation
            || !Arc::ptr_eq(&saga.session, &self.session)
            || self.connection.is_cancelled()
        {
            self.finish_retry(None, Some("Retry cancelled".into()), false)
                .await;
            return;
        }
        let messages = self.agent.raw_messages().await;
        if !saga.native_mode {
            let receipts = match self.session.projector.current_body_receipts(&messages) {
                Ok(receipts) => receipts,
                Err(error) => {
                    self.finish_retry(None, Some(error.to_string()), false)
                        .await;
                    self.drain_queues = false;
                    return;
                }
            };
            if receipts != saga.expected_messages {
                self.finish_retry(
                    None,
                    Some("Retry continuation failed locally: the active context changed during backoff".into()),
                    false,
                )
                .await;
                self.drain_queues = false;
                return;
            }
        }
        let native_check = (|| -> Result<()> {
            if let Some(receipt) = &saga.native_context {
                receipt.recheck(
                    &messages,
                    saga.session
                        .values
                        .as_ref()
                        .context("Native retry values missing")?,
                )?;
            }
            if let Some(guard) = &saga.native_guard {
                guard.recheck(self)?;
            }
            Ok(())
        })();
        if let Err(error) = native_check {
            self.finish_retry(None, Some(error.to_string()), false)
                .await;
            self.drain_queues = false;
            return;
        }
        let saga = self.retry.as_mut().unwrap();
        saga.deadline = None;
        let command = Command {
            id: saga.command.id.clone(),
            kind: saga.command.kind.clone(),
            frame: saga.command.frame.clone(),
        };
        if let Err(error) = self.start(None, command).await {
            self.finish_retry(
                None,
                Some(format!("Retry continuation failed locally: {error}")),
                false,
            )
            .await;
            self.drain_queues = false;
        }
    }

    async fn begin_header_continue(
        &mut self,
        active: &ActiveRun,
        hit: HeaderInterruption,
    ) -> Result<bool> {
        if self.input_closed
            || self.connection.is_cancelled()
            || hit.generation != self.prompt_generation
            || !Arc::ptr_eq(&active.sink.session, &self.session)
        {
            return Ok(false);
        }
        let target_timestamp = match &hit.target {
            HeaderTarget::Legacy(timestamp) => *timestamp,
            HeaderTarget::Native(target) => {
                return self
                    .begin_header_continue_native(active, hit.clone(), target.clone())
                    .await;
            }
        };
        let mut messages = self.agent.raw_messages().await;
        let Some(message) = raw_last_assistant(&messages)?.filter(|message| {
            message.timestamp == target_timestamp
                && message.stop_reason == ara_ai::StopReason::Aborted
        }) else {
            return Ok(false);
        };
        // Retain the existing visible/image/tool/unknown-effect veto. A header
        // count is not evidence that an earlier tool has had no side effect.
        if super::rpc_host_retry::disposition(
            &message,
            &[Message::Assistant(message.clone())],
            &ara_ai::retry_classification::RetryClass {
                abort: true,
                ..Default::default()
            },
        ) != Some(super::rpc_host_retry::RetryDisposition::RemoveFailed)
        {
            return Ok(false);
        }
        let entry_id = active
            .sink
            .entries
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find_map(|(id, candidate)| {
                (candidate.as_assistant() == Some(&message)).then(|| id.clone())
            })
            .context("Gemini interrupted assistant has no durable native entry ID")?;
        let notice = LoopGuardNotice::gemini_headers(hit.headers);
        // Persist the exact branch exclusion and notice before adopting live
        // context. Failure leaves the original aborted leaf selected.
        let notice_id = self
            .session
            .journal
            .lock()
            .await
            .append_gemini_reminder_after_aborted(&notice, &entry_id)?;
        messages.pop();
        let raw = self
            .session
            .projector
            .new_custom_input(notice.event_message())?;
        self.session.projector.record_entry_id(&raw, notice_id);
        messages.push(raw);
        self.agent.replace_idle_raw_messages(messages.clone())?;
        let failed = wire(
            AgentEvent::MessageEnd {
                message: Message::Assistant(message).into(),
            }
            .full()?["message"]
                .clone(),
        );
        let public = notice.event_message();
        {
            let mut transcript = self.session.messages.lock().unwrap();
            if let Some(index) = transcript
                .iter()
                .rposition(|candidate| candidate.deep_equal(&failed))
            {
                transcript.remove(index);
            }
            transcript.push(wire(public.clone()));
        }
        self.output
            .frame(json!({"type":"message_start","message":public}));
        self.output
            .frame(json!({"type":"message_end","message":public}));
        self.header_continue = Some(PendingHeaderContinue {
            session: self.session.clone(),
            generation: hit.generation,
            command: Command {
                id: active.command.id.clone(),
                kind: active.command.kind.clone(),
                frame: active.command.frame.clone(),
            },
            expected_messages: self.session.projector.current_body_receipts(&messages)?,
            deadline: Instant::now() + Duration::from_millis(1),
        });
        self.auto_compaction_pending = false;
        Ok(true)
    }

    async fn resume_header_continue(&mut self) {
        let Some(pending) = self.header_continue.take() else {
            return;
        };
        if self.input_closed
            || self.connection.is_cancelled()
            || pending.generation != self.prompt_generation
            || !Arc::ptr_eq(&pending.session, &self.session)
        {
            self.finish_retry(None, Some("Gemini continuation cancelled".into()), false)
                .await;
            return;
        }
        let receipts = match self
            .session
            .projector
            .current_body_receipts(&self.agent.raw_messages().await)
        {
            Ok(receipts) => receipts,
            Err(error) => {
                self.finish_retry(None, Some(error.to_string()), false)
                    .await;
                self.drain_queues = false;
                self.output
                    .response(&pending.command, None, Some(error.to_string()));
                return;
            }
        };
        if receipts != pending.expected_messages {
            self.finish_retry(
                None,
                Some("Gemini continuation cancelled: the active context changed".into()),
                false,
            )
            .await;
            self.drain_queues = false;
            self.output.response(
                &pending.command,
                None,
                Some("Gemini continuation cancelled: the active context changed".into()),
            );
            return;
        }
        if let Err(error) = self.start(None, pending.command).await {
            self.finish_retry(
                None,
                Some(format!("Gemini continuation failed locally: {error}")),
                false,
            )
            .await;
            self.drain_queues = false;
            self.output.frame(
                json!({"type":"notice","level":"warning","source":"loop-guard",
                "message":format!("Gemini tool-call reminder continuation failed: {error}")}),
            );
        }
    }

    async fn compact(&mut self, focus: Option<&str>) -> Result<WireValue> {
        if focus.is_some_and(|focus| focus.len() > 1_000_000) {
            bail!("compaction instructions exceed the summary input limit");
        }
        let drain = self.drain_queues;
        self.is_compacting = true;
        self.abort().await;
        // Fixed manual compaction selects remote/snapcompact/soft in the
        // configured order. Handoff and shake belong to automatic recovery.
        let result = async {
            let settings = self.compaction_policy.recovery_settings()?;
            for method in &settings.method_order {
                if method == "remote" {
                    let remote = Box::pin(self.remote_history(
                        focus,
                        ara_agent::compaction::SummaryOptions {
                            max_tokens: self.config.max_tokens,
                            ..Default::default()
                        },
                    ))
                    .await;
                    match remote {
                        Ok(Some(summary)) => return Ok(wire(summary)),
                        Ok(None) => {}
                        Err(error)
                            if self.connection.is_cancelled() || ara_cli::remote_compaction::is_cancelled(&error) =>
                        {
                            return Err(error);
                        }
                        Err(_) => self.output.frame(json!({"type":"notice","level":"warning","source":"compaction",
                            "message":"Remote compaction failed; trying the next preferred method"})),
                    }
                } else if method == "snapcompact" && focus.is_none_or(str::is_empty) {
                    match Box::pin(self.snapcompact_history(&[])).await {
                        Ok(Some(summary)) => return Ok(summary),
                        Ok(None) => {}
                        Err(error) if self.maintenance_stop_requested() || ara_cli::handoff::is_cancelled(&error) => {
                            return Err(error);
                        }
                        Err(error) => {
                            self.output.frame(json!({"type":"notice","level":"warning","source":"compaction",
                            "message":format!("Snapcompact failed ({error}); trying the next preferred method")}))
                        }
                    }
                } else if method == "soft" {
                    // Keep large split/fold state off the shared command future.
                    return Box::pin(self.compact_with_options(
                        focus,
                        ara_agent::compaction::SummaryOptions {
                            max_tokens: self.config.max_tokens,
                            ..Default::default()
                        },
                    ))
                    .await
                    .map(wire);
                }
            }
            bail!("No available manual compaction method is configured")
        }
        .await;
        self.is_compacting = false;
        self.auto_compaction_pending = false;
        self.drain_queues = drain && !self.maintenance_stop_requested();
        result
    }

    async fn compact_with_options(
        &mut self,
        focus: Option<&str>,
        options: ara_agent::compaction::SummaryOptions,
    ) -> Result<Value> {
        use ara_agent::compaction::{
            select_native_entry_compaction_cut, summarize_native_entry_compaction_cut,
            summary_output_budget_tokens,
        };
        use ara_agent::tokenizer::MessageCountOptions;
        if self.session.persistence_error.lock().unwrap().is_some() {
            bail!("session persistence failed; restart from the journal before compacting");
        }
        if focus.is_some_and(|focus| focus.len() > 1_000_000) {
            bail!("compaction instructions exceed the summary input limit");
        }
        // Compaction keeps Agent queues; explicit abort normally suppresses
        // autonomous draining, so restore the previous intent after cleanup.
        let drain = self.drain_queues;
        self.is_compacting = true;
        self.abort().await;
        let result = async {
            if self.connection.is_cancelled() {
                bail!("Request was aborted");
            }
            let current = self.agent.raw_messages().await;
            // Check recovery/admission before billing or changing the journal.
            self.agent.replace_idle_raw_messages(current.clone())?;
            let snapshot = self
                .session
                .journal
                .lock()
                .await
                .native_projected_compaction_snapshot()?;
            let previous = snapshot
                .previous_summary
                .as_ref()
                .map(|summary| summary.previous_summary_for_text_compaction());
            let previous = previous.as_deref();
            let sources = ara_cli::native_compaction::sources_for_model(
                &snapshot.entries,
                &self.config.model,
            );
            let cut = select_native_entry_compaction_cut(
                &sources,
                self.sessions.args.compact_keep_tokens,
                previous,
            )
            .map_err(|error| anyhow::anyhow!("{error}"))?
            .context(
                "No native message prefix can be compacted with the current keep-token budget",
            )?;
            let tokens_before =
                self.budget_message_tokens(&current, MessageCountOptions::default())? as u64;
            let seconds = self.max_time.unwrap_or(120.0).clamp(0.0, 120.0);
            let deadline = Instant::now() + Duration::from_secs_f64(seconds);
            let settings = self.compaction_policy.recovery_settings()?;
            summary_output_budget_tokens(settings.reserve_tokens)?;
            let accepted = summarize_native_entry_compaction_cut(
                &sources,
                &cut,
                previous,
                focus,
                &self.config.model,
                self.config.provider.as_ref(),
                settings.reserve_tokens,
                options,
                deadline,
                &self.connection,
            )
            .await
            .map_err(|error| anyhow::anyhow!("Compaction summary failed: {error}"))?;
            let mut journal = self.session.journal.lock().await;
            let old_leaf = journal.leaf_id().map(str::to_owned);
            if let Err(error) = journal.commit_native_entry_compaction(
                &snapshot,
                &accepted.text,
                &cut.first_kept_entry_id,
                &accepted.window_source_entry_ids,
                tokens_before,
            ) {
                if matches!(error, ara_session::CompactionCommitError::Storage(_)) {
                    self.session
                        .persistence_error
                        .lock()
                        .unwrap()
                        .get_or_insert_with(|| error.to_string());
                    self.connection.cancel();
                }
                return Err(error.into());
            }
            if let Err(error) = self.adopt_route_context(&journal).await {
                self.session
                    .persistence_error
                    .lock()
                    .unwrap()
                    .get_or_insert_with(|| error.to_string());
                self.connection.cancel();
                return Err(error);
            }
            *self.session.messages.lock().unwrap() = public_messages_after_publication(
                &journal,
                &self.session,
                &self.connection,
                &mut self.drain_queues,
            )?;
            // Reader-only Bash remains independent during the summary. Hold
            // admission while changing its original owner's branch target.
            let mut dispatcher_target = self.bash_dispatcher.current.lock().unwrap();
            for target in self.bash_targets.iter().filter_map(Weak::upgrade) {
                let mut target = target.lock().unwrap();
                if Arc::ptr_eq(&target.session, &self.session)
                    && matches!(target.destination, BashDestination::Current)
                {
                    target.destination = BashDestination::Branch {
                        parent: old_leaf.clone(),
                    };
                }
            }
            let target = Arc::new(Mutex::new(BashTarget {
                session: self.session.clone(),
                destination: BashDestination::Current,
            }));
            *dispatcher_target = target.clone();
            self.bash_targets
                .retain(|target| target.strong_count() != 0);
            self.bash_targets.push(Arc::downgrade(&target));
            self.bash_target = target;
            // The rewritten context invalidates stateful provider replay.
            self.config.provider = self.sessions.provider.build();
            if let Some(controller) = &self.sessions.context_controller {
                controller.fresh_provider_session();
            }
            self.publish_snapshot(&self.config);
            Ok(
                json!({"summary":accepted.text,"firstKeptEntryId":cut.first_kept_entry_id,
                "tokensBefore":tokens_before}),
            )
        }
        .await;
        self.is_compacting = false;
        self.auto_compaction_pending = false;
        self.drain_queues = drain && !self.connection.is_cancelled();
        result
    }

    async fn drain_pending_auto_compaction(&mut self, eof: bool) -> bool {
        // Preserve the serial loop's busy -> take -> eof short-circuit order.
        if !self.auto.busy() && std::mem::take(&mut self.auto_compaction_pending) && !eof {
            self.maybe_auto_compact(&[]).await
        } else {
            true
        }
    }

    async fn maybe_auto_compact(&mut self, pending: &[AgentMessage]) -> bool {
        self.maybe_auto_compact_before_start(pending, false).await
    }

    async fn maybe_auto_compact_for_restore(&mut self, pending: &[AgentMessage]) -> bool {
        self.maybe_auto_compact_before_start(pending, true).await
    }

    async fn maybe_auto_compact_before_start(
        &mut self,
        pending: &[AgentMessage],
        restored: bool,
    ) -> bool {
        use ara_agent::tokenizer::MessageCountOptions;
        let threshold = if restored {
            let reserve = self
                .compaction_policy
                .recovery_settings()
                .ok()
                .and_then(|settings| settings.reserve_tokens);
            let window =
                maintenance::retry_context_capacity(self.config.model.context_window, reserve)
                    .map(|window| window.floor() as usize)
                    .unwrap_or(self.sessions.args.compact_threshold);
            if self.sessions.args.compact_threshold == 0 {
                window
            } else {
                window.min(self.sessions.args.compact_threshold)
            }
        } else {
            self.sessions.args.compact_threshold
        };
        let threshold=self.child_compaction_threshold(threshold).await;
        if self.active.is_some()
            || (!restored
                && (self.retry.is_some()
                    || self.header_continue.is_some()
                    || self.maintenance_continue.is_some()))
            || self.maintenance.is_some()
            || self.connection.is_cancelled()
        {
            return true;
        }
        let messages = self.agent.raw_messages().await;
        let tokens = match self.context_tokens(&messages, pending).await {
            Ok(tokens) => tokens,
            Err(error) => {
                self.output.frame(
                    json!({"type":"notice","level":"warning","source":"compaction","message":error.to_string()}),
                );
                self.drain_queues = false;
                return false;
            }
        };
        let freed = match self.prune_local_history().await {
            Ok(freed) => freed,
            Err(error) => {
                self.output.frame(
                    json!({"type":"notice","level":"warning","source":"compaction","message":error.to_string()}),
                );
                return true;
            }
        };
        if !self.compaction_required(tokens,threshold,restored) {
            return true;
        }
        let key = {
            let journal = self.session.journal.lock().await;
            (
                journal.session_id().to_owned(),
                journal.leaf_id().map(str::to_owned),
                tokens,
            )
        };
        if self.auto_compaction_checked.as_ref() == Some(&key) {
            return true;
        }
        let initial_leaf = key.1.clone();
        self.auto_compaction_checked = Some(key);
        let settings = self.compaction_policy.recovery_settings();
        // Fixed snapcompact-first vision policy; other method decisions remain
        // owned by the existing loop. Read the normalized settings only once.
        let action = if settings.as_ref().is_ok_and(|settings| {
            settings
                .method_order
                .first()
                .is_some_and(|method| method == "snapcompact")
        }) && ara_cli::snapcompact::supports_images(
            self.sessions.provider.metadata.as_ref(),
        ) {
            "snapcompact"
        } else {
            "context-full"
        };
        self.output
            .frame(json!({"type":"auto_compaction_start","reason":"threshold","action":action}));
        let mut no_progress = false;
        let mut native_no_preparation_dead_end = false;
        let result = async {
            let settings = settings?;
            let floor = self
                .non_message_tokens()
                .saturating_add(self.budget_message_tokens(
                    &self.agent.raw_messages().await,
                    MessageCountOptions { exclude_encrypted_reasoning: true },
                )?)
                .saturating_add(self.budget_message_tokens(pending, MessageCountOptions { exclude_encrypted_reasoning: true })?);
            let mut progress = tokens.saturating_sub(freed).max(floor) <= (threshold as f64 * 0.8).floor() as usize;
            let mut ran_shake = false;
            for method in &settings.method_order {
                if progress {
                    break;
                }
                if method == "remote" {
                    let remote = Box::pin(self.remote_history(
                        None,
                        ara_agent::compaction::SummaryOptions {
                            oneshot_retry: None,
                            max_tokens: self.config.max_tokens,
                        },
                    ))
                    .await;
                    let summary = match remote {
                        Ok(Some(summary)) => summary,
                        Ok(None) => continue,
                        Err(error)
                            if self.connection.is_cancelled() || ara_cli::remote_compaction::is_cancelled(&error) =>
                        {
                            return Err(error);
                        }
                        Err(_) => {
                            self.output.frame(json!({"type":"notice","level":"warning","source":"compaction",
                                "message":"Remote compaction failed; trying the next preferred method"}));
                            continue;
                        }
                    };
                    progress = self.recovery_fits(settings.reserve_tokens, Some((threshold, 0)), pending).await?;
                    if !progress {
                        progress = self
                            .rescue_local_history(
                                settings.reserve_tokens,
                                Some((threshold, 0)),
                                ran_shake,
                                pending,
                                None,
                            )
                            .await?;
                    }
                    if !progress {
                        no_progress = true;
                        bail!("Remote compaction could not create recovery headroom; automatic continuation stopped");
                    }
                    if self.maintenance_stop_requested() {
                        return Err(ara_cli::handoff::cancelled());
                    }
                    return Ok::<_, anyhow::Error>(wire(summary));
                } else if method == "snapcompact" {
                    if !ara_cli::snapcompact::supports_images(self.sessions.provider.metadata.as_ref()) { continue; }
                    let attempt = Box::pin(self.snapcompact_history(pending)).await;
                    let summary = match attempt {
                        Ok(Some(summary)) => Some(summary),
                        Ok(None) => None,
                        Err(error) if self.maintenance_stop_requested() || ara_cli::handoff::is_cancelled(&error) => {
                            return Err(error);
                        }
                        Err(error) => {
                            self.output.frame(json!({"type":"notice","level":"warning","source":"compaction",
                                "message":format!("Snapcompact unavailable ({error}); trying the next preferred method")}));
                            continue;
                        }
                    };
                    // Successful None proves native absent/zero cut; budget and
                    // renderer Err still follow the unchanged fallback branch.
                    progress = self.recovery_fits(settings.reserve_tokens, Some((threshold, 0)), pending).await?;
                    if !progress {
                        progress = self.rescue_local_history(settings.reserve_tokens, Some((threshold, 0)),
                            ran_shake, pending, None).await?;
                    }
                    // Original post-event Done is persisted after the old archive.
                    // Only rescue can append the new trailing compaction, so inspect
                    // the selected native leaf after the recovery tiers settle.
                    let no_preparation_tail = if !progress && summary.is_none() {
                        let journal = self.session.journal.lock().await;
                        let snapshot = journal.native_snapcompact_snapshot()?;
                        journal.branch().last().is_some_and(|entry|
                            entry.id == snapshot.projection.leaf_id && entry.kind == "compaction")
                    } else {
                        false
                    };
                    if !progress && no_preparation_tail {
                        native_no_preparation_dead_end = true;
                        // Tier zero already stamped a rebuilt archive when applicable.
                        // Fixed maintenance stops here instead of attempting soft on
                        // the same trailing compaction with no summarizable prefix.
                        no_progress = true;
                        bail!("Snapcompact could not create recovery headroom; automatic continuation stopped");
                    }
                    if let Some(summary) = summary {
                        if !progress {
                            no_progress = true;
                            let latest = self.session.journal.lock().await.native_snapcompact_snapshot()?
                                .projection.previous_summary.map(|summary| summary.entry_id);
                            if let Some(entry_id) = latest {
                                self.stamp_frame_dead_end(&entry_id, false).await?;
                            }
                            bail!("Snapcompact could not create recovery headroom; automatic continuation stopped")
                        }
                        return Ok::<_, anyhow::Error>(summary);
                    }
                } else if method == "handoff" {
                    let handoff = Box::pin(self.handoff_local_history(Some(ara_cli::handoff::AUTO_FOCUS), true)).await;
                    let summary = match handoff {
                        Ok(Some(summary)) => summary,
                        Ok(None) => continue,
                        Err(error) if self.connection.is_cancelled() || ara_cli::handoff::is_cancelled(&error) => {
                            return Err(error);
                        }
                        Err(_) => {
                            self.output.frame(json!({"type":"notice","level":"warning","source":"compaction",
                                "message":"Handoff compaction failed; trying the next preferred method"}));
                            continue;
                        }
                    };
                    progress = self.recovery_fits(settings.reserve_tokens, Some((threshold, 0)), pending).await?;
                    if !progress {
                        progress = self
                            .rescue_local_history(
                                settings.reserve_tokens,
                                Some((threshold, 0)),
                                ran_shake,
                                pending,
                                None,
                            )
                            .await?;
                    }
                    if !progress {
                        no_progress = true;
                        bail!("Handoff could not create recovery headroom; automatic continuation stopped");
                    }
                    return Ok::<_, anyhow::Error>(wire(summary));
                } else if method == "shake" {
                    ran_shake = true;
                    self.shake_local_history(false, None).await?;
                    progress = self.recovery_fits(settings.reserve_tokens, Some((threshold, freed)), pending).await?;
                } else if method == "soft" {
                    let summary = Box::pin(self.compact_with_options(
                        None,
                        ara_agent::compaction::SummaryOptions {
                            oneshot_retry: None,
                            max_tokens: self.config.max_tokens,
                        },
                    ))
                    .await?;
                    progress = self.recovery_fits(settings.reserve_tokens, Some((threshold, 0)), pending).await?;
                    if !progress {
                        progress = self
                            .rescue_local_history(
                                settings.reserve_tokens,
                                Some((threshold, 0)),
                                ran_shake,
                                pending,
                                None,
                            )
                            .await?;
                    }
                    if !progress {
                        no_progress = true;
                        bail!("Compaction could not create recovery headroom; automatic continuation stopped");
                    }
                    return Ok::<_, anyhow::Error>(wire(summary));
                }
            }
            if !progress {
                progress = self
                    .rescue_local_history(settings.reserve_tokens, Some((threshold, 0)), ran_shake, pending, None)
                    .await?;
            }
            if !progress {
                no_progress = true;
                bail!("Local maintenance could not create recovery headroom; automatic continuation stopped");
            }
            Ok(wire(json!({"method":"shake","tokensAfter":self.context_tokens(&self.agent.raw_messages().await, pending).await?})))
        }
        .await;
        let cancelled = result
            .as_ref()
            .err()
            .is_some_and(ara_cli::remote_compaction::is_cancelled);
        let ready = result.is_ok() && !self.connection.is_cancelled();
        let mut event = wire(json!({"type":"auto_compaction_end","action":action,
            "aborted":self.connection.is_cancelled() || cancelled,"willRetry":false}));
        match result {
            Ok(result) => event.insert("result", result),
            Err(_)
                if native_no_preparation_dead_end
                    && !cancelled
                    && !self.maintenance_stop_requested()
                    && self.session.persistence_error.lock().unwrap().is_none() => {}
            Err(error) => event.insert(
                "errorMessage",
                wire(json!(super::sanitize_text(&error.to_string()))),
            ),
        }
        if no_progress && !self.maintenance_stop_requested() {
            let journal = self.session.journal.lock().await;
            if journal.leaf_id() != initial_leaf.as_deref()
                && let Ok(snapshot) = journal.native_snapcompact_snapshot()
                && let Some(summary) = snapshot
                    .projection
                    .previous_summary
                    .filter(|summary| summary.has_archive())
            {
                let published = (|| -> Result<WireValue> {
                    let mut published = wire(
                        json!({"summary":summary.summary,"shortSummary":summary.short_summary,
                        "method":"snapcompact","entryId":summary.entry_id,"firstKeptEntryId":summary.first_kept_entry_id,
                        "tokensBefore":summary.tokens_before}),
                    );
                    if let Some(preserve) = summary.preserve_data_without_archive_utf16()? {
                        published.insert("preserveData", WireValue::parse(preserve.get())?);
                    }
                    Ok(published)
                })();
                match published {
                    Ok(published) => event.insert("result", published),
                    Err(error) => {
                        // Publication already changed the journal. Do not retry
                        // compaction when its public receipt cannot be formed.
                        self.session
                            .persistence_error
                            .lock()
                            .unwrap()
                            .get_or_insert_with(|| error.to_string());
                        self.connection.cancel();
                        self.drain_queues = false;
                        event.insert("aborted", WireValue::Bool(true));
                        event.insert(
                            "errorMessage",
                            wire(json!(super::sanitize_text(&error.to_string()))),
                        );
                    }
                }
            }
        }
        if native_no_preparation_dead_end
            && !self.maintenance_stop_requested()
            && event.get("errorMessage").is_none()
        {
            event.insert("skipped", WireValue::Bool(event.get("result").is_none()));
        }
        self.output.send(OutputItem::Frame(event));
        if no_progress && !self.maintenance_stop_requested() {
            let journal = self.session.journal.lock().await;
            let published_warning = (journal.leaf_id() != initial_leaf.as_deref())
                .then(|| journal.native_snapcompact_snapshot().ok())
                .flatten()
                .and_then(|snapshot| snapshot.projection.previous_summary)
                .and_then(|summary| {
                    journal
                        .entries()
                        .iter()
                        .find(|entry| entry.id == summary.entry_id)
                })
                .and_then(|entry| entry.raw["warning"].as_str())
                .filter(|warning| {
                    warning.starts_with("Compaction freed too little context to make progress")
                });
            let warning = published_warning
                .map(str::to_owned)
                .unwrap_or_else(|| snapcompact::dead_end_warning(None));
            self.output.frame(
                json!({"type":"notice","level":"warning","source":"compaction","message":warning}),
            );
        }
        ready && !cancelled && !self.connection.is_cancelled()
    }

    async fn append_bash(&mut self, pending: PendingBash) -> Result<()> {
        let (session, destination) = {
            let target = pending.target.lock().unwrap();
            (target.session.clone(), target.destination.clone())
        };
        match destination {
            BashDestination::Current => {
                if !Arc::ptr_eq(&session, &self.session) {
                    bail!("Bash current destination no longer owns the active Session");
                }
                let raw = session
                    .projector
                    .new_legacy_message(pending.message.event_message(), None)?;
                self.agent.append_idle_raw_message(raw.clone())?;
                let mut journal = session.journal.lock().await;
                let id = journal.append_bash_execution(&pending.message)?;
                session.projector.record_entry_id(&raw, id);
                session.messages.lock().unwrap().push(wire(raw.json()?));
                self.extend_maintenance_context(&session, &raw)?;
                if let Some(retry) = &mut self.retry
                    && retry.generation == self.prompt_generation
                    && Arc::ptr_eq(&retry.session, &session)
                {
                    if let Some(receipt) = &mut retry.native_context {
                        receipt.append(&raw)?;
                    } else {
                        retry.expected_messages.extend(
                            session
                                .projector
                                .current_body_receipts(std::slice::from_ref(&raw))?,
                        );
                    }
                }
            }
            BashDestination::Detached { parent } | BashDestination::Branch { parent } => {
                let mut journal = session.journal.lock().await;
                let id =
                    journal.append_bash_execution_to_branch(&pending.message, parent.as_deref())?;
                let mut target = pending.target.lock().unwrap();
                match &mut target.destination {
                    BashDestination::Detached { parent } | BashDestination::Branch { parent } => {
                        *parent = Some(id)
                    }
                    BashDestination::Current => {
                        unreachable!("serial Host owns destination transitions")
                    }
                }
            }
        }
        Ok(())
    }

    fn bash_persistence_failed(&mut self, target: &Arc<Mutex<BashTarget>>, error: &anyhow::Error) {
        let session = target.lock().unwrap().session.clone();
        let message =
            format!("Bash receipt could not be recorded; effects may have occurred: {error:#}");
        *session.persistence_error.lock().unwrap() = Some(message.clone());
        self.bash_error = Some(message.clone());
        eprintln!("ara: {message}");
        self.connection.cancel();
    }

    async fn flush_pending_bash(&mut self) {
        if self.active.is_some() {
            return;
        }
        for pending in std::mem::take(&mut self.pending_bash) {
            if self.maintenance.is_some()
                && matches!(
                    pending.target.lock().unwrap().destination,
                    BashDestination::Current
                )
            {
                self.pending_bash.push(pending);
                continue;
            }
            let target = pending.target.clone();
            if let Err(error) = self.append_bash(pending).await {
                self.bash_persistence_failed(&target, &error);
            }
        }
    }

    async fn completed_bash(&mut self, (id, result): BashCompletion) {
        let job = self
            .bash_dispatcher
            .jobs
            .lock()
            .unwrap()
            .remove(&id)
            .expect("tracked Bash completion");
        let _ = job.task.await;
        let result = match result {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => {
                self.output.response(&job.command, None, Some(error.0));
                return;
            }
            Err(error) => {
                self.output.response(
                    &job.command,
                    None,
                    Some(format!("Bash task failed; effects may be unknown: {error}")),
                );
                return;
            }
        };
        let data = bash_result_value(&result);
        let message = BashExecutionMessage {
            command: job.text,
            output: result.output_with_status_notice(),
            exit_code: result.exit_code,
            cancelled: result.cancelled,
            truncated: result.truncated,
            meta: bash_output_meta_from_summary(&data),
            timestamp: chrono::Utc::now().timestamp_millis(),
            exclude_from_context: None,
        };
        let defer = {
            let target = job.target.lock().unwrap();
            matches!(target.destination, BashDestination::Current)
                && Arc::ptr_eq(&target.session, &self.session)
                && (self.active.is_some() || self.maintenance.is_some())
        };
        let pending = PendingBash {
            target: job.target.clone(),
            message,
        };
        if defer {
            self.pending_bash.push(pending);
        } else if let Err(error) = self.append_bash(pending).await {
            self.output
                .response(&job.command, None, Some(error.to_string()));
            self.bash_persistence_failed(&job.target, &error);
            return;
        }
        self.output.response(&job.command, Some(data), None);
    }

    async fn overlay(
        &self,
        definitions: &[HostToolDefinition],
        active_names: &[String],
    ) -> Result<super::ToolOverlay> {
        let peer_task_depth=super::rpc_host_settings::peer_task_depth(Some(&*self.session.journal.lock().await))?;
        let active = active_names
            .iter()
            .filter_map(|name| definitions.iter().find(|tool| tool.name == *name).cloned())
            .collect::<Vec<_>>();
        let tools = self.tool_bridge.adapters(&active);
        // Retain every registered owner, including hidden/unadvertised entries.
        // Only active adapters enter tools; the Native registry keeps all owners.
        let registered_adapters = self.tool_bridge.adapters(definitions);
        let registered_owners = self.tool_bridge.native_owners(definitions);
        Ok(super::ToolOverlay {
            peer_coordination:self.sessions.peers.is_some(),
            peer_task_depth,peer_enable_irc:Some(self.sessions.peer_messaging_enabled),
            child_read_summarize:None,
            child_restrict_lsp:false,
            read_host: self.sessions.tool_owner.read_host.clone(),
            read_video: self.sessions.tool_owner.read_video.clone(),
            tools,
            native_owners: registered_adapters
                .into_iter()
                .zip(registered_owners)
                .collect(),
            labels: active
                .into_iter()
                .map(|tool| (tool.name, tool.label))
                .collect(),
        })
    }

    fn active_host_names(&self) -> Vec<String> {
        self.config
            .tools
            .iter()
            .map(|tool| tool.definition().name.clone())
            .filter(|name| self.host_tools.iter().any(|tool| tool.name == *name))
            .collect()
    }

    fn base_config(&self) -> AgentConfig {
        let mut config = self.config.clone();
        config.tools.retain(|tool| {
            !self
                .host_tools
                .iter()
                .any(|host| host.name == tool.definition().name)
        });
        config
    }

    fn retained_session(
        &self,
        header: &Value,
        file: Option<&std::path::Path>,
    ) -> Option<Arc<Session>> {
        self.bash_targets
            .iter()
            .filter_map(Weak::upgrade)
            .find_map(|target| {
                let session = target.lock().unwrap().session.clone();
                let closed=session.tool_owner.lock().unwrap().native.as_ref().is_some_and(|native|native.wait_session.is_closed());
                (!closed&&same_session_identity(&session, header, file)).then_some(session)
            })
    }

    fn fresh_actual_session(&self) -> Result<Option<PreparedActualSession>> {
        let factory = match self.sessions.actual_values_factory.as_ref() {
            Some(factory) => factory,
            None if self.session.values.is_none() => return Ok(None),
            None => bail!("Host actual Session values factory is missing"),
        };
        // Capture values while holding only each target's existing short lock.
        // The Host callback executes after every target/session guard is released.
        let retained = self
            .bash_targets
            .iter()
            .filter_map(Weak::upgrade)
            .filter_map(|target| {
                let session = target.lock().unwrap().session.clone();
                session.values.clone()
            })
            .collect::<Vec<_>>();
        let fresh = factory()?;
        if let Some(owner) = fresh.native_session() {
            owner.check_values(fresh.values())?;
            // A fresh factory returns an unpublished draft. Original async
            // setup supplies its tools before the serve/adopt admission gate.
        }
        if self
            .session
            .values
            .iter()
            .chain(retained.iter())
            .any(|old| {
                Arc::ptr_eq(old, fresh.values())
                    || old.heap().heap_token() == fresh.values().heap().heap_token()
            })
        {
            bail!("Host actual Session values factory reused a retained Session binding or heap");
        }
        Ok(Some(fresh))
    }

    async fn session_config(
        &self,
        reset: bool,
        values: Option<Arc<SessionValues>>,
        native_session: Option<Arc<ara_cli::native_factory::PreparedNativeSession>>,
        peer_depth: Option<usize>,
    ) -> Result<PreparedSessionConfig> {
        let mut overlay = self.overlay(&self.host_tools, &self.active_host_names()).await?;
        if let Some(depth)=peer_depth{overlay.peer_task_depth=depth;}
        let retained_owner = native_session.as_ref().and_then(|selected| {
            std::iter::once(self.session.clone())
                .chain(
                    self.bash_targets
                        .iter()
                        .filter_map(Weak::upgrade)
                        .map(|target| target.lock().unwrap().session.clone()),
                )
                .find(|session| {
                    session
                        .native_session
                        .as_ref()
                        .is_some_and(|owner| Arc::ptr_eq(owner, selected))
                })
                .map(|session| session.tool_owner.lock().unwrap().clone())
        });
        self.sessions
            // Explicit adoption refreshes Skill metadata even when an unchanged
            // transcript keeps its provider/store. Model-only reconfiguration
            // instead supplies the existing Skill snapshot below.
            .config(
                &self.base_config(),
                self.sessions.provider.metadata.as_ref(),
                None,
                reset,
                values,
                native_session,
                overlay,
                retained_owner,
            )
            .await
    }

    fn publish_snapshot(&self, config: &AgentConfig) {
        if let Some(policy) = &self.sessions.tool_owner.read_video_policy {
            policy.publish_model(
                &config.model,
                self.sessions.provider.metadata.as_ref(),
                config
                    .tools
                    .iter()
                    .any(|tool| tool.definition().name == "inspect_image"),
            );
        }
        if let Some(read) = &self.sessions.tool_owner.read_session {
            read.publish_model(
                &config.model,
                self.sessions.provider.metadata.as_ref(),
                config
                    .tools
                    .iter()
                    .any(|tool| tool.definition().name == "inspect_image"),
            );
        }
        if let Ok(settings) = self.compaction_policy.recovery_settings() {
            self.session.projector.set_donor(
                &config.model,
                self.sessions.provider.metadata.as_ref(),
                &settings.remote,
                self.sessions.provider.route.remote_supports_images(),
            );
        } else {
            self.session.projector.clear_donor();
        }
        let candidate = ExecutionSnapshot::from_config(config);
        if let Some(controller) = &self.sessions.context_controller {
            self.snapshot_publication.publish(
                controller,
                candidate,
                self.sessions.provider.metadata.as_ref(),
            );
        } else {
            self.snapshot_publication.publish_unbound(candidate);
        }
        self.route_generation.store(
            self.sessions.provider.route.generation(),
            std::sync::atomic::Ordering::Release,
        );
    }

    #[cfg(feature="test-fixture")]
    fn cold_adoption_binding(&self) -> Value {
        let reset = self.sessions.provider.reset_host.as_ref().map(|host| {
            let state = host.current.lock().unwrap();
            json!({"epoch":state.epoch,"sessionIds":state.session_ids,
                "provider":state.provider,"model":state.model_id,"closed":state.closed})
        });
        let snapshot_provider = {
            let snapshot = self.snapshot.read().unwrap();
            snapshot.model.as_ref().map(|model| Arc::as_ptr(&model.provider) as *const () as usize)
        };
        json!({"sessionId":self.session.header["id"],"reset":reset,
            "configProvider":Arc::as_ptr(&self.config.provider) as *const () as usize,
            "snapshotProvider":snapshot_provider,
            "routeGeneration":self.route_generation.load(std::sync::atomic::Ordering::Acquire)})
    }

    fn publish_native_catalog(&self,publication:Option<&super::native_setup::NativeSetupPublication>)->Result<()> {
        if let Some(publication)=publication{
            let active=self.active.as_ref().and_then(|run|run.native.as_ref());
            let snapshot=publication.pending_snapshot.clone();
            #[cfg(feature="test-fixture")]
            let snapshot={
                let mut fault=self.cold_publication_fault.lock().unwrap();
                if let Some(fault)=fault.as_mut(){
                    fault.publication_attempted=true;
                    fault.snapshot.clone()
                }else{snapshot}
            };
            // Even the controlled fault reaches the actual prepared publisher.
            // A foreign realm is rejected before old catalog/grants are changed.
            let published=publication.prepared.publish_catalog_snapshot(snapshot,active);
            #[cfg(feature="test-fixture")]
            if let Some(fault)=self.cold_publication_fault.lock().unwrap().as_mut(){
                fault.native_error=published.as_ref().err().map(ToString::to_string);
            }
            published?;
        }Ok(())
    }
    fn publish_native_preparation(&self,publication:Option<&super::native_setup::NativeSetupPublication>)->Result<()> {
        // Same-ID refresh prepares the binding without mutating the original
        // Wait/Ingress/service. Adoption supplies its different activation
        // ticket and calls the catalog-only primitive instead.
        let binding=match(publication,self.sessions.peers.as_ref(),self.peer_runtime.as_ref()){
            (Some(publication),Some(peers),Some(runtime))=>Some(peers.prepare_native(&publication.native_owner,runtime,self.session.file.is_some())?),
            _=>None,
        };
        if let (Some(publication),Some(dispatch))=(publication,&self.task_dispatch){
            super::task_host::bind(dispatch,&publication.native_owner,self.session.header["id"].as_str().context("Task Session id missing")?,&self.sessions.cwd,self.sessions.tool_owner.edit_session.clone())?;
        }
        self.publish_native_catalog(publication)?;
        if let Some(binding)=binding{binding.commit();}Ok(())
    }

    fn publish_native_after_journal(
        &self,
        publication: Option<&super::native_setup::NativeSetupPublication>,
        session: &Arc<Session>,
    ) -> Result<()> {
        if let Err(error) = self.publish_native_preparation(publication) {
            // A durable model receipt already exists. Keep old membership and
            // stop the connection rather than use a mismatched live route.
            session
                .persistence_error
                .lock()
                .unwrap()
                .get_or_insert_with(|| {
                    format!("Native publication failed after journal commit: {error}")
                });
            self.connection.cancel();
            return Err(error);
        }
        Ok(())
    }


    fn peer_state(&self)->ara_cli::host_peers::NativePeerState {
        let native=self.sessions.tool_owner.native.as_ref();let child=native.and_then(|owner|owner.child_contract.as_ref());
        ara_cli::host_peers::NativePeerState{running:self.active.is_some()||self.auto.busy()||self.retry.is_some()||self.maintenance.is_some()||self.header_continue.is_some()||self.maintenance_continue.is_some(),plan_mode:self.sessions.tool_owner.context.as_ref().and_then(|context|context.edit_host()).is_some_and(|host|host.path_policy().plan_active),disposed:self.connection.is_cancelled(),dispatch_active:child.is_some_and(|owner|owner.dispatch_active()),pending_publication:child.is_some_and(|owner|owner.desired_snapshot().generation!=owner.published_snapshot().generation||owner.publication_error().is_some()),reply_obligation:self.peer_wake.is_some(),owned_jobs:native.is_some_and(|owner|owner.bash_session.effects().pending_count()!=0||owner.bash_session.snapshots().iter().any(|job|job.status=="running"||job.result.is_none()))}
    }
    async fn prepare_peer_adoption(&self,session:&Arc<Session>,owner:&super::CliToolOwner)->Result<Option<PreparedPeerActivation>> {
        let(Some(mut peers),Some(sender))=(self.sessions.peers.clone(),self.peer_sender.clone())else{return Ok(None)};
        let journal=session.journal.lock().await;let id=journal.session_id().to_owned();let persistent=journal.is_persistent();
        let parent=journal.latest_child_init()?.and_then(|init|init.extra.get("araChildParentId").and_then(Value::as_str).map(str::to_owned));
        let root=if parent.is_none(){id.clone()}else{self.sessions.peer_root.clone().context("Child lost parent namespace")?};let peer_depth=super::rpc_host_settings::peer_task_depth(Some(&*journal))?;drop(journal);
        let reuse=self.peer_runtime.as_ref().filter(|runtime|runtime.peer.id==id&&runtime.peer.root==root).cloned();
        if reuse.is_none(){if let Some(old)=&self.peer_runtime{
            if peers.coordinator.registry.get_exact(&old.peer).is_some_and(|record|record.kind==ara_cli::host_peers::PeerKind::Main&&record.parent_id.is_none()){
                // Keep the original actual Session/runtime installed until the
                // old Peer namespace has its exact combined known receipt.
                // Detached Bash producers keep their original separate owner.
                let receipt=peers.release_root(old).await?;anyhow::ensure!(receipt.settled,"Root ownership transition has unresolved descendant effects");
                peers=peers.successor(receipt)?;
            }
        }}
        let attachment=peers.prepare_attachment(&root,&id,parent,session.file.clone(),sender,self.sessions.peer_attachment.clone().unwrap_or(ara_cli::host_peers::AttachmentExpectation::Absent),reuse)?;
        let uri_peer=peers.uri(&root,&id,session.file.clone(),self.sessions.messaging_eligibility(peer_depth));
        let fallback=super::prepared_host_uri(owner.vault.as_ref(),owner.uri_base.as_ref(),Some(&uri_peer),None);
        let router=ara_cli::session_artifacts::ArtifactUriRouter::new(session.artifacts.clone(),fallback);
        Ok(Some(PreparedPeerActivation{attachment,root,persistent,uri_peer,router}))
    }
    async fn peer_create(&mut self,mut init:ara_session::PersistedChildInit,context:String,structured:bool)->Result<ara_cli::host_peers::RuntimeKey>{
        let peers=self.sessions.peers.clone().context("This Host has no Peer capability")?;let runtime=self.peer_runtime.clone().context("Current Peer owner missing")?;
        if init.resolved_model.is_none(){let model=self.sessions.provider.route.model();init.resolved_model=Some(format!("{}/{}",model.provider,model.id));}
        let prompt=ara_cli::child_contract::prompt::ChildPromptInputs{base:init.system_prompt.clone(),agent:init.agent.clone().unwrap_or_default(),context,structured,..Default::default()};
        let child={let mut journal=self.session.journal.lock().await;journal.materialize()?;let factory=super::child_host::ChildHostFactory::new(&self.sessions,&journal)?;factory.create(init,prompt,structured,&self.connection).await?};
        peers.attach_child_in_root(&runtime.peer.root,child)
    }
    async fn peer_revive(&mut self,record:ara_cli::host_peers::PeerRecord)->Result<ara_cli::host_peers::RuntimeKey>{
        let current=self.peer_runtime.as_ref().context("Revive parent not bound")?;anyhow::ensure!(record.key.root==current.peer.root&&record.parent_id.as_deref()==Some(current.peer.id.as_str()),"Cold revival parent/root differs from current Native owner");
        let peers=self.sessions.peers.clone().context("Peer Host missing")?;anyhow::ensure!(peers.coordinator.registry.get_exact(&record.key).is_some(),"Peer ref replaced before cold revival");
        let file=record.session_file.as_ref().context("Peer has no persisted transcript")?;
        let child={let journal=self.session.journal.lock().await;let factory=super::child_host::ChildHostFactory::new(&self.sessions,&journal)?;match peers.task_resource(&record.key){Some(lease)=>factory.open_warm_isolated(record.clone(),lease,&self.connection).await?,None=>factory.open_peer(record.clone(),&self.connection).await?}};
        peers.attach_child_in_root(&record.key.root,child)
    }
    async fn append_peer_idle(&mut self,body:Value)->Result<String>{
        let raw=self.session.projector.new_custom_input(body)?;
        // Agent's non-awaiting idle lock rejects an unpaired tool call. No URI or model IO occurs here.
        self.agent.append_idle_raw_message(raw.clone())?;
        let result={let mut journal=self.session.journal.lock().await;self.session.projector.append_current_body(&mut journal,&raw).and_then(|id|{journal.materialize()?;Ok(id)})};
        let id=match result{Ok(id)=>id,Err(error)=>{self.session.persistence_error.lock().unwrap().get_or_insert_with(||format!("Peer input persistence failed after context adoption: {error}"));self.connection.cancel();return Err(error.into());}};
        self.session.messages.lock().unwrap().push(wire(raw.json()?));self.extend_maintenance_context(&self.session.clone(),&raw)?;Ok(id)
    }
    async fn existing_parent_receipt(&self,completion:&ara_cli::host_peers::WakeCompletion)->Result<Option<ara_cli::host_peers::ParentAdoption>>{
        // Reconcile the exact original journal receipt before every possible retry.
        let key=serde_json::to_value(&completion.job)?;let runtime=serde_json::to_value(&completion.runtime)?;
        {let mut journal=self.session.journal.lock().await;
            let prior=journal.entries().iter().find(|entry|entry.observed_message_field(&["details","peerWakeJob"]).ok().flatten().and_then(|body|ara_session::native_checked_serde(&body).ok()).as_ref()==Some(&key)&&entry.observed_message_field(&["details","peerWakeRuntime"]).ok().flatten().and_then(|body|ara_session::native_checked_serde(&body).ok()).as_ref()==Some(&runtime)).map(|entry|entry.id.clone());
            if let Some(entry_id)=prior{journal.materialize()?;return Ok(Some(ara_cli::host_peers::ParentAdoption{session_id:journal.session_id().into(),entry_id,persistent:journal.is_persistent()&&journal.is_on_disk()}));}
        }
        Ok(None)
    }
    async fn parent_receipt(&mut self,completion:&ara_cli::host_peers::WakeCompletion)->Result<ara_cli::host_peers::ParentAdoption>{
        let current=self.peer_runtime.as_ref().context("Parent has no Native owner")?;
        anyhow::ensure!(completion.job.parent_id==current.peer.id&&completion.job.child.root==current.peer.root,"Parent completion targets another actual owner");
        if let Some(receipt)=self.existing_parent_receipt(completion).await?{return Ok(receipt);}
        #[cfg(feature="test-fixture")]
        super::peer_host::fixture_frontier("parent-before-adopt",&serde_json::to_string(&completion.job)?).await?;
        let entry_id=self.append_peer_idle(super::peer_host::parent_result_body(completion)).await?;
        #[cfg(feature="test-fixture")]
        super::peer_host::fixture_frontier("parent-after-materialize-before-ack",&serde_json::to_string(&completion.job)?).await?;
        let journal=self.session.journal.lock().await;Ok(ara_cli::host_peers::ParentAdoption{session_id:journal.session_id().into(),entry_id,persistent:journal.is_persistent()&&journal.is_on_disk()})
    }
    async fn flush_peer_parent_results(&mut self){
        let ingress=self.sessions.tool_owner.native.as_ref().map(|native|native.peer_ingress.clone());
        if let Some(ingress)=ingress{while let Some(pending)=ingress.take_parent(){let result=self.parent_receipt(&pending.completion).await.map(super::peer_host::PeerActorReply::Adoption);let _=pending.reply.send(result);}}
    }
    async fn peer_request(&mut self,request:super::peer_host::PeerActorRequest){
        use super::peer_host::{PeerActorOp as Op,PeerActorReply as Reply};
        if self.peer_runtime.as_ref()!=Some(&request.runtime){let _=request.reply.send(Err(anyhow::anyhow!("Peer actor generation replaced before admission")));return;}
        let operation=match request.operation {Op::Wake(wake,cancel)=>{
            let pooled=self.sessions.tool_owner.native.as_ref().and_then(|owner|owner.child_contract.as_ref()).is_some_and(|owner|!owner.desired_snapshot().items.is_empty());
            if self.peer_wake.is_some()||self.peer_state().running||self.peer_state().pending_publication||pooled {
                self.peer_deferred.push_back(super::peer_host::PendingWake{request:wake,cancel,reply:request.reply,outcome:None,turn:None});return;
            }
            if cancel.is_cancelled(){let _=request.reply.send(Err(anyhow::anyhow!("Peer wake cancelled before admission")));return;}
            let input=self.session.projector.new_custom_input(json!({"role":"custom","customType":"peer-wake","content":wake.rendered.join("\n\n"),"display":true,"attribution":"agent","timestamp":ara_cli::host_peers::now_ms(),"details":ara_cli::host_peers::native::incoming_batch_details(&wake.messages,self.peer_runtime.as_ref().and_then(|runtime|self.sessions.peers.as_ref()?.coordinator.registry.get_exact(&runtime.peer)).and_then(|record|record.parent_id).as_deref(),self.peer_runtime.as_ref().and_then(|runtime|self.sessions.peers.as_ref()?.coordinator.registry.get_exact(&runtime.peer)).is_some_and(|record|record.kind!=ara_cli::host_peers::PeerKind::Main))}));
            match input {
                Ok(input)=>{self.peer_wake=Some(super::peer_host::PendingWake{request:wake,cancel,reply:request.reply,outcome:None,turn:None});let command=Command::new(wire(json!({"type":"__peer_wake"})));if let Err(error)=self.start(Some(input),command).await{if let Some(pending)=self.peer_wake.take(){let _=pending.reply.send(Err(error));}}},
                Err(error)=>{let _=request.reply.send(Err(error.into()));},
            }return;
        },operation=>operation};
        let operation=match operation{Op::ParentResult(completion)=>{
            match self.existing_parent_receipt(&completion).await {Ok(Some(receipt))=>{let _=request.reply.send(Ok(Reply::Adoption(receipt)));return;},Err(error)=>{let _=request.reply.send(Err(error));return;},Ok(None)=>{}}
            if self.peer_state().running {if let Some(native)=&self.sessions.tool_owner.native{native.peer_ingress.enqueue_parent(super::peer_host::PendingParentResult{completion,reply:request.reply});return;}}
            let result=self.parent_receipt(&completion).await.map(Reply::Adoption);let _=request.reply.send(result);return;
        },operation=>operation};
        let result:Result<Reply>=async{match operation {
            Op::Inspect=>Ok(Reply::State(self.peer_state())),
            Op::RelayObservation(message)=>{
                // A display card is never admitted into Agent, projector, Journal,
                // inbox, wake or activity. Recheck after serial actor consumption.
                let peers=self.sessions.peers.as_ref().context("Relay owner missing")?;
                anyhow::ensure!(self.sessions.peer_root.as_deref()==Some(request.runtime.peer.root.as_str())&&peers.coordinator.relay_main_is_current(&request.runtime)&&message.from!=request.runtime.peer.id&&message.to!=request.runtime.peer.id,"Relay Main/root generation changed before display");
                self.output.frame(json!({"type":"irc_message","message":ara_cli::host_peers::main_relay_record(&message)}));
                Ok(Reply::Delivered)
            },
            #[cfg(feature="test-fixture")]
            Op::FixturePlan(policy)=>{self.sessions.tool_owner.edit_session.as_ref().context("Actual plan owner missing")?.bind_protocol_services(None,policy)?;Ok(Reply::Delivered)},
            #[cfg(feature="test-fixture")]
            Op::FixtureUriSchemes(schemes)=>{self.uri_bridge.set_schemes(&schemes).map_err(anyhow::Error::msg)?;Ok(Reply::Delivered)},
            #[cfg(feature="test-fixture")]
            Op::FixtureRefreshHostTools=>{let definitions=self.host_tools.clone();self.set_host_tools(definitions).await?;Ok(Reply::Delivered)},
            #[cfg(feature="test-fixture")]
            Op::FixtureNewSession(parent)=>{self.new_session(parent.as_deref()).await?;Ok(Reply::Delivered)},
            Op::Deliver(delivery)=>{
                anyhow::ensure!(delivery.message.to==request.runtime.peer.id,"Peer delivery recipient changed");
                let running=self.peer_state().running;
                #[cfg(feature="test-fixture")]
                let delivery_identity=format!("{}:{}:{}",request.runtime.peer.id,delivery.message.id,delivery.message.wake_relay);
                let record=self.sessions.peers.as_ref().and_then(|peers|peers.coordinator.registry.get_exact(&request.runtime.peer)).context("Peer ingress owner vanished")?;
                let details=ara_cli::host_peers::native::incoming_details(&delivery.message,record.parent_id.as_deref()==Some(delivery.message.from.as_str()),running,record.kind!=ara_cli::host_peers::PeerKind::Main&&!delivery.message.wake_relay);
                match delivery.route {
                    ara_cli::host_peers::DeliveryRoute::ParentSteer=>{anyhow::ensure!(running,"Parent steer recipient became idle");let body=json!({"role":"custom","customType":"irc","content":delivery.rendered,"display":true,"attribution":"agent","timestamp":delivery.message.ts,"details":details});let input=self.session.projector.new_custom_input(body)?;self.agent.steer_raw_with_source(input,ara_agent::SteeringSource::Agent)?;self.drain_queues=true;},
                    ara_cli::host_peers::DeliveryRoute::SiblingInterrupt=>{anyhow::ensure!(running,"IRC interrupt recipient became idle");self.sessions.peers.as_ref().context("Peer capability removed")?.queues.interrupt(&request.runtime,delivery);},
                    ara_cli::host_peers::DeliveryRoute::PlanAppend=>{anyhow::ensure!(!running&&self.peer_state().plan_mode,"Plan append recipient changed state");self.append_peer_idle(json!({"role":"custom","customType":"irc","content":delivery.rendered,"display":true,"attribution":"agent","timestamp":delivery.message.ts,"details":details})).await?;},
                }
                #[cfg(feature="test-fixture")]
                super::peer_host::fixture_frontier("peer-after-delivery-before-ack",&delivery_identity).await?;
                Ok(Reply::Delivered)
            },
            Op::Revive(record)=>Ok(Reply::Runtime(self.peer_revive(record).await?)),
            Op::Cancel(turn)=>{
                if self.peer_wake.as_ref().is_some_and(|wake|wake.request.job.turn==turn){if let Some(wake)=&self.peer_wake{wake.cancel.cancel();}self.abort().await;}
                else if let Some(wake)=self.peer_deferred.iter().find(|wake|wake.request.job.turn==turn){wake.cancel.cancel();}
                // The library has already cancelled this exact job token. A not-yet-
                // admitted wake will observe it; never abort a competing ordinary Run.
                Ok(Reply::Delivered)
            },
            Op::Close=>{self.abort().await;self.connection.cancel();Ok(Reply::Delivered)},
            Op::Wake(..)|Op::ParentResult(..)=>unreachable!(),
        }}.await;let _=request.reply.send(result);
    }
    async fn start_deferred_peer(&mut self){
        if self.peer_wake.is_some()||self.peer_state().running||self.peer_state().pending_publication{return;}
        if self.sessions.tool_owner.native.as_ref().and_then(|owner|owner.child_contract.as_ref()).is_some_and(|owner|!owner.desired_snapshot().items.is_empty()){return;}
        while let Some(pending)=self.peer_deferred.pop_front(){
            if pending.cancel.is_cancelled(){let _=pending.reply.send(Err(anyhow::anyhow!("Deferred Peer wake cancelled before admission")));continue;}
            if self.peer_runtime.as_ref()!=Some(&pending.request.runtime){let _=pending.reply.send(Err(anyhow::anyhow!("Deferred Peer wake owner was replaced; nothing dispatched")));continue;}
            let runtime=pending.request.runtime.clone();self.peer_request(super::peer_host::PeerActorRequest{runtime,operation:super::peer_host::PeerActorOp::Wake(pending.request,pending.cancel),reply:pending.reply}).await;break;
        }
    }
    async fn finish_peer_wake(&mut self){
        let Some(mut pending)=self.peer_wake.take()else{return};let mut outcome=pending.outcome.take().unwrap_or_else(||ara_cli::host_peers::WakeOutcome::failed("Peer idle without retained Native terminal receipt"));
        outcome.cancelled|=pending.cancel.is_cancelled();
        let observed:Result<()>=async{
            if let Some(turn)=&pending.turn {
                turn.require_settled()?;let native=self.sessions.tool_owner.native.as_ref().context("Peer Native owner missing at finalization")?;
                let effects=native.bash_session.effects();let receipts=effects.settle().await;
                anyhow::ensure!(!receipts.iter().any(|receipt|receipt.uncertain()),"Peer owned effects have unknown outcomes");
                let all_settled=native.bash_session.pending_deliveries().is_empty();
                let observer=turn.observer()?;let terminal_yield=observer.has_terminal_yield();
                let result=turn.finalize(all_settled)?;
                outcome.accepted_yield=terminal_yield&&result.accepted;
                if terminal_yield {outcome.accepted=result.accepted;if let Some(error)=result.error.as_ref(){if !outcome.cancelled&&outcome.error.is_none(){outcome.error=Some(error.clone());}}}
                else if matches!(result.status,ara_cli::child_contract::ChildResultStatus::PendingOwnedWork|ara_cli::child_contract::ChildResultStatus::Aborted){outcome.accepted=false;if outcome.error.is_none(){outcome.error=result.error.clone();}}
                // Clean conversational wake has no new yield obligation. The
                // original raw terminal text is authoritative; prior artifact stays.
                else {outcome.accepted=outcome.accepted&&!outcome.cancelled&&outcome.error.is_none()&&outcome.answer.as_deref().is_some_and(|answer|!answer.trim().is_empty());}
                if outcome.accepted_yield {
                    if !pending.request.job.parent_id.is_empty(){self.sessions.peers.as_ref().context("Peer Host disappeared before accepted yield")?.coordinator.jobs.register(pending.request.job.clone(),pending.request.runtime.clone())?;}
                    let value=result.data.context("Accepted yield has no actual result")?;let values=native.prepared.values();
                    let encoded=values.heap().stringify(&value,ara_rpc::value_heap::StringifyMode::Plain,None,values.runtime())?.context("Accepted yield data is undefined")?.to_utf8().context("Peer artifact UTF16 projection")?;
                    let data:Value=serde_json::from_str(&encoded)?;let answer=if let Some(text)=data.as_str(){text.to_owned()}else{serde_json::to_string_pretty(&data)?};
                    let transcript=self.session.file.as_ref().context("Accepted Peer yield has no persistent transcript")?;
                    let paths=ara_cli::host_peers::roster::artifact_paths(transcript,&pending.request.runtime.peer.id);
                    let file=paths.markdown;
                    let temp=file.with_extension(format!("{}.tmp",uuid::Uuid::now_v7()));tokio::fs::write(&temp,&answer).await?;tokio::fs::rename(&temp,&file).await?;
                    let sidecar=paths.json;let sidecar_temp=sidecar.with_extension(format!("{}.tmp",uuid::Uuid::now_v7()));tokio::fs::write(&sidecar_temp,&encoded).await?;tokio::fs::rename(&sidecar_temp,&sidecar).await?;
                    outcome.answer=Some(answer);outcome.output_path=Some(file);
                }
            }
            if outcome.answer.as_ref().is_none_or(|answer|answer.trim().is_empty())&&!outcome.accepted_yield&&outcome.error.is_none(){outcome.error=Some("Peer wake completed without output".into());}
            Ok(())
        }.await;
        if let Err(error)=observed{outcome.error=Some(match outcome.error{Some(primary)=>format!("{primary}; finalization: {error}"),None=>format!("Peer finalization failed: {error}")});outcome.accepted=false;}
        if let Some(peers)=&self.sessions.peers{peers.coordinator.registry.finish_wake(&pending.request.runtime,pending.request.job.turn,outcome.accepted_yield&&outcome.accepted,outcome.response_at);}
        // Reply is sent only after exact Native settlement, persistence and before-idle parent registration.
        let _=pending.reply.send(Ok(super::peer_host::PeerActorReply::Wake(outcome)));
    }

    async fn child_action(&mut self,action:NativeChildAction,command:&Command)->Result<Value>{
        match action {
            NativeChildAction::Install{session_id,ticket,snapshot}=>{
                if self.session.header["id"].as_str()!=Some(session_id.as_str()){bail!("Child Session generation replaced before install");}
                if self.active.is_some()||self.auto.busy()||self.maintenance.is_some(){bail!("Child contract install requires actual idle ownership");}
                let owner=self.sessions.tool_owner.native.as_ref().and_then(|n|n.child_contract.clone()).context("Not a child Native Session")?;
                let mut journal=self.session.journal.lock().await;
                let mut init=journal.latest_child_init()?.context("Child session_init missing during install")?;
                let mut prompt=super::child_host::prompt_for_snapshot(&init,&snapshot,owner.config().structured)?;
                if init.extra.contains_key("araChildPromptBase") {
                    let native=self.sessions.tool_owner.native.as_ref().context("Actual child Native owner missing")?;
                    let docs=native.snapshot.device_prompt_docs(native.prepared.values(),native.prepared.values().runtime())?;
                    if !docs.is_empty(){prompt.push(docs.to_utf8().context("Child device prompt UTF16 boundary")?);}
                }
                init.system_prompt=prompt.clone();
                init.work_pool_yield_items=(!snapshot.items.is_empty()).then(||snapshot.items.iter().map(|i|ara_session::ChildWorkPoolItem{id:i.id.clone(),index:i.index}).collect());
                let mut config=self.config.clone();config.system_prompt=prompt.clone();
                let result=owner.with_current_publication(&ticket,||{
                    let id=journal.append_child_init(&init).map_err(|e|ara_cli::child_contract::ChildContractError(format!("Child init publication failed; inspect journal before replay: {e}")))?;
                    self.publish_snapshot(&config);
                    Ok(id)
                })?;
                drop(journal);
                self.config=config;
                Ok(json!({"generation":snapshot.generation,"systemPrompt":prompt,"journalEntry":result}))
            }
            NativeChildAction::Prompt{session_id,text,turn}=>{
                if self.session.header["id"].as_str()!=Some(session_id.as_str()){bail!("Child Session replaced before followup");}
                if self.active.is_some()||self.auto.busy()||self.maintenance.is_some(){
                    if self.sessions.tool_owner.native.as_ref().and_then(|native|native.child_contract.as_ref()).is_some_and(|owner|owner.published_snapshot().generation==turn.expected_generation){
                        turn.reject_busy_not_dispatched(session_id.clone(),turn.expected_generation,self.active.as_ref().and_then(|run|run.child_turn.as_ref()).and_then(|turn|turn.dispatch_id()));
                    }
                    bail!("Child followup lost idle admission; nothing dispatched");
                }
                let input=text.map(|text|{
                    let body=if turn.is_continuation(){
                        // OMP b91 task/executor.ts synthetic yield reminder ->
                        // agent-session.ts developer prompt; yield report lookup
                        // must see the canonical role, not a custom projection.
                        json!({"role":"developer","content":[{"type":"text","text":text}],"attribution":"agent","synthetic":true,"timestamp":chrono::Utc::now().timestamp_millis()})
                    }else{
                        json!({"role":"custom","customType":"child-assignment","content":text,"display":false,"attribution":"agent","timestamp":chrono::Utc::now().timestamp_millis()})
                    };
                    self.session.projector.new_custom_input(body)
                }).transpose()?;
                self.child_turn=Some(turn.clone());
                if let Err(error)=self.start(input,Command{id:command.id.clone(),kind:command.kind.clone(),frame:command.frame.clone()}).await {
                    self.child_turn=None;turn.fail(error.to_string());return Err(error);
                }
                Ok(json!({"admitted":true,"sessionId":session_id}))
            }
        }
    }

    async fn set_host_tools(
        &mut self,
        definitions: Vec<HostToolDefinition>,
    ) -> Result<Vec<String>> {
        let names = definitions
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<Vec<_>>();
        let unique = names.iter().collect::<std::collections::HashSet<_>>();
        if unique.len() != names.len() {
            bail!("RPC host tool names must be unique");
        }
        let base = self.base_config();
        for name in &names {
            if super::TOOL_NAMES.contains(&name.as_str())
                || base
                    .tools
                    .iter()
                    .any(|tool| tool.definition().name == *name)
            {
                bail!("RPC host tool \"{name}\" conflicts with an existing tool");
            }
        }
        let mut active = self
            .active_host_names()
            .into_iter()
            .filter(|name| unique.contains(name))
            .collect::<Vec<_>>();
        active.extend(
            definitions
                .iter()
                .filter(|tool| {
                    !tool.hidden
                        && !self
                            .host_tools
                            .iter()
                            .any(|previous| previous.name == tool.name)
                })
                .map(|tool| tool.name.clone()),
        );
        let overlay = self.overlay(&definitions, &active).await?;
        let retained_owner = self.session.tool_owner.lock().unwrap().clone();
        // Prepare the complete prompt and adapters before publishing either.
        // Pending calls retain their old adapter and original Session sink.
        let candidate = self
            .sessions
            .config(
                &base,
                self.sessions.provider.metadata.as_ref(),
                Some(&self.skills),
                false,
                self.session.values.clone(),
                self.session.native_session.clone(),
                overlay,
                Some(retained_owner),
            )
            .await?;
        self.publish_native_preparation(candidate.native_publication.as_ref())?;
        *self.session.tool_owner.lock().unwrap() = candidate.tool_owner.clone();
        self.session
            .bind_edit_session(&candidate.tool_owner)
            .await?;
        self.sessions.tool_owner = candidate.tool_owner;
        self.sessions.context_controller = candidate.context_controller;
        self.config = candidate.config;
        self.skills = candidate.skills;
        self.publish_snapshot(&self.config);
        self.host_tools = definitions;
        Ok(names)
    }

    async fn refresh_acp_plan(&mut self, decision: &NativePlanRefresh) -> Result<()> {
        if self.sessions.acp_plan.is_none() {
            bail!("Native Session has no ACP plan owner");
        }
        let overlay = self.overlay(&self.host_tools, &self.active_host_names()).await?;
        let retained_owner = self.session.tool_owner.lock().unwrap().clone();
        let candidate = self
            .sessions
            .config(
                &self.base_config(),
                self.sessions.provider.metadata.as_ref(),
                Some(&self.skills),
                false,
                self.session.values.clone(),
                self.session.native_session.clone(),
                overlay,
                Some(retained_owner),
            )
            .await?;
        // Finish every fallible/journal-await preparation before the ticket
        // gate. Ordinary RPC keeps its established set_host_tools wrapper.
        if let Some(edit) = &candidate.tool_owner.edit_session {
            let journal = self.session.journal.lock().await;
            edit.bind(&journal, self.session.artifacts.clone())?;
            if self
                .session
                .edit_session
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|prior| !Arc::ptr_eq(prior, edit))
            {
                bail!("Session tools cannot replace their EDIT diagnostics owner");
            }
        }
        let mut state = decision
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match &*state {
            NativePlanRefreshState::Pending => {}
            NativePlanRefreshState::Cancelled(reason) => bail!(reason.clone()),
            NativePlanRefreshState::Committed(result) => {
                return result.clone().map_err(anyhow::Error::msg);
            }
        }
        // No await or fallible EDIT bind follows Native publication. Cancel
        // takes this same short gate: pending cancellation prevents publish;
        // a published snapshot always retains its success receipt even when
        // epoch/EOF arrives before the asynchronous reply is consumed.
        let result: Result<()> = (|| {
            self.publish_native_preparation(candidate.native_publication.as_ref())?;
            *self.session.tool_owner.lock().unwrap() = candidate.tool_owner.clone();
            if let Some(edit) = &candidate.tool_owner.edit_session {
                *self.session.edit_session.lock().unwrap() = Some(edit.clone());
            }
            self.sessions.tool_owner = candidate.tool_owner;
            self.sessions.context_controller = candidate.context_controller;
            self.config = candidate.config;
            self.skills = candidate.skills;
            self.publish_snapshot(&self.config);
            Ok(())
        })();
        *state = NativePlanRefreshState::Committed(
            result.as_ref().map(|_| ()).map_err(ToString::to_string),
        );
        result
    }

    fn available_commands(&self) -> Vec<Value> {
        // Fixed available-commands.ts:59-68 advertises the registered Skill
        // snapshot, including hidden entries. Body loading belongs to prompt
        // admission; a metadata query must not reopen the source file.
        self.skills
            .iter()
            .map(|skill| {
                let description = if skill.description.is_empty() {
                    format!("Run {} skill", skill.name)
                } else {
                    skill.description.clone()
                };
                json!({"name":format!("skill:{}", skill.name),"description":description,
                    "input":{"hint":"arguments"},"source":"skill"})
            })
            .collect()
    }

    fn emit_available_commands(&self) {
        self.output.frame(
            json!({"type":"available_commands_update","commands":self.available_commands()}),
        );
    }

    async fn adopt(
        &mut self,
        journal: Option<SessionJournal>,
        header: Value,
        messages: Vec<AgentMessage>,
        candidate: PreparedSessionConfig,
    ) -> Result<()> {
        let PreparedSessionConfig {
            values,
            native_session,
            native_publication,
            mut config,
            skills,
            mut tool_owner,
            context_controller,
        } = candidate;
        // Preparation is complete and the old owned Run is joined. Commit the
        // replacement once; each settled RunSink retains its original journal.
        let previous_leaf = self
            .session
            .journal
            .lock()
            .await
            .leaf_id()
            .map(str::to_owned);
        let proposed_id = header["id"].as_str().context("Session identity is missing")?;
        let agent_context = self.sessions.agent_context.as_ref().map(|context| context.for_primary(proposed_id));
        let primary_provider = agent_context.as_ref().map(|_| self.sessions.provider.prepare_primary_rebind(proposed_id, agent_context.clone()));
        #[cfg(feature="test-fixture")]
        if let Some(fault)=self.cold_publication_fault.lock().unwrap().as_mut(){
            // This receipt is emitted only after the actual preparation above;
            // it is not evidence that publication or adoption succeeded.
            fault.prepared_primary=Some(json!({"actualId":proposed_id,
                "primaryId":primary_provider.as_ref().and_then(|provider|provider.agent_context.as_ref())
                    .map(|context|context.auth_scope.primary_session_id.clone()),
                "oldBinding":self.cold_adoption_binding()}));
        }
        let proposed = Session::new_with_agent_context(
            journal,
            header,
            &messages,
            values.clone(),
            native_session.clone(),
            agent_context.clone(),
        )?;
        let new_leaf = proposed.journal.lock().await.leaf_id().map(str::to_owned);
        let targets = self
            .bash_targets
            .iter()
            .filter_map(Weak::upgrade)
            .collect::<Vec<_>>();
        // A still-owned Bash may retain the same file that switch just opened.
        // Reuse its journal lock instead of leaving a stale snapshot writer.
        let retained = targets.iter().find_map(|target| {
            let session = target.lock().unwrap().session.clone();
            let closed=session.tool_owner.lock().unwrap().native.as_ref().is_some_and(|native|native.wait_session.is_closed());
            (!closed&&same_session_file(&session, &proposed)).then_some(session)
        });
        let target = retained.as_ref().unwrap_or(&proposed);
        if let Some(expected) = &agent_context {
            let actual = target.agent_context.as_ref().context("Retained Session lost agent context")?;
            anyhow::ensure!(actual.identity == expected.identity && actual.auth_scope.primary_session_id == expected.auth_scope.primary_session_id
                && actual.auth_scope.oauth_account_pools == expected.auth_scope.oauth_account_pools, "Retained Session agent scope differs from candidate");
        }
        if !same_session_values(&values, &target.values) {
            bail!("Session values do not match the retained adoption target");
        }
        if !same_native_session(&native_session, &target.native_session) {
            bail!("Native Session owner does not match the retained adoption target");
        }
        target.bind_edit_session(&tool_owner).await?;
        if let (Some(native),Some(dispatch))=(&tool_owner.native,&self.task_dispatch){
            super::task_host::bind(dispatch,native,target.header["id"].as_str().context("Adopted Task Session id missing")?,&self.sessions.cwd,tool_owner.edit_session.clone())?;
        }
        validate_session_values(&config, context_controller.as_ref(), &values)?;
        validate_native_session_hooks(&config, &values, &native_session)?;
        let id = target.header["id"]
            .as_str()
            .context("Session identity is missing")?
            .to_owned();
        let messages = ara_cli::remote_compaction::route_raw_context(
            &*proposed.journal.lock().await,
            &proposed.projector,
            &config.model,
            self.sessions.provider.metadata.as_ref(),
            &self.compaction_policy.recovery_settings()?.remote,
            self.sessions.provider.route.remote_supports_images(),
        )?;
        // All fallible journal/context/Agent preparation precedes membership,
        // retained-writer, Bash routing and URI publication.
        let agent = Agent::new_raw(config.clone(), messages)?;
        agent.set_steering_mode(self.agent.steering_mode());
        agent.set_follow_up_mode(self.agent.follow_up_mode());
        config.interrupt_mode = self.agent.interrupt_mode();
        agent.set_interrupt_mode(config.interrupt_mode);
        let peer_activation=self.prepare_peer_adoption(target,&tool_owner).await?;
        // Acquire the existing retained journal swap locks before short native
        // binding tickets. No sync guard crosses an await or failed publish.
        let retained_journal_owner=retained.clone();
        let proposed_journal_owner=retained.as_ref().map(|_|proposed.clone());
        let mut retained_journal=if let Some(owner)=&retained_journal_owner{Some(owner.journal.lock().await)}else{None};
        let mut proposed_journal=if let Some(owner)=&proposed_journal_owner{Some(owner.journal.lock().await)}else{None};
        let native_for_activation=tool_owner.native.clone();
        {
            let native_binding=match(&peer_activation,native_for_activation.as_ref()){
                (Some(activation),Some(native))=>Some(activation.attachment.owner.prepare_native(native,&activation.attachment.runtime,activation.persistent)?),
                _=>None,
            };
            self.publish_native_catalog(native_publication.as_ref())?;
            // No sync guard survives this lexical block or crosses an await.
            if let Some(binding)=native_binding{binding.commit();}
        }
        // From here through actual membership/URI publication, only infallible
        // synchronous commits remain. The serial actor processes no callback.
        // All fallible preparation/publication is complete. Clearing a staged
        // candidate earlier would erase the live owner's tags on rollback.
        // /clear and same-ID refresh preserve the store; new/switch/branch IDs
        // clear the issuing tool store and the adopted tool-side owner.
        if target.header["id"] != self.session.header["id"] {
            if let Some(previous) = &self.sessions.tool_owner.context {
                previous.edit_store.clear();
            }
            if let Some(current) = &tool_owner.context {
                current.edit_store.clear();
            }
        }
        let session = if let Some(retained) = retained {
            std::mem::swap(
                &mut **retained_journal.as_mut().expect("prepared retained journal lock"),
                &mut **proposed_journal.as_mut().expect("prepared proposed journal lock"),
            );
            *retained.messages.lock().unwrap() = proposed.messages.lock().unwrap().clone();
            *retained.name.lock().unwrap() = proposed.name.lock().unwrap().clone();
            *retained.partial.lock().unwrap() = None;
            *retained.persistence_error.lock().unwrap() = None;
            retained.failed_recovery_message.lock().unwrap().take();
            retained
        } else {
            proposed
        };
        drop(retained_journal);drop(proposed_journal);
        for target in &targets {
            let mut target = target.lock().unwrap();
            target.destination = match &target.destination {
                BashDestination::Current if Arc::ptr_eq(&target.session, &self.session) => {
                    if Arc::ptr_eq(&target.session, &session) {
                        if previous_leaf == new_leaf {
                            BashDestination::Current
                        } else {
                            BashDestination::Branch {
                                parent: previous_leaf.clone(),
                            }
                        }
                    } else {
                        BashDestination::Detached {
                            parent: previous_leaf.clone(),
                        }
                    }
                }
                BashDestination::Detached { parent } | BashDestination::Branch { parent }
                    if Arc::ptr_eq(&target.session, &session) =>
                {
                    BashDestination::Branch {
                        parent: parent.clone(),
                    }
                }
                destination => destination.clone(),
            };
        }
        let bash_target = Arc::new(Mutex::new(BashTarget {
            session: session.clone(),
            destination: BashDestination::Current,
        }));
        self.bash_targets
            .retain(|target| target.strong_count() != 0);
        self.bash_targets.push(Arc::downgrade(&bash_target));
        *self.bash_dispatcher.current.lock().unwrap() = bash_target.clone();
        self.bash_target = bash_target;
        if let Some(provider) = primary_provider {
            self.sessions.provider.adopt_primary_rebind(provider);
            self.sessions.agent_context = agent_context;
        }
        if config.model.provider == "openai-codex" || self.sessions.provider.account_auth.is_some()
        {
            config.provider = self.sessions.provider.bind_codex_session(id, None);
        } else {
            self.sessions
                .provider
                .admit_reset_session(&config.model, Some(id.clone()), Some(id));
        }
        if let Some(activation)=peer_activation {
            let old=self.sessions.peers.clone().zip(self.peer_runtime.clone());
            tool_owner.uri_peer=Some(activation.uri_peer);
            if let Some(context)=&tool_owner.context{context.set_uri_port(activation.router.clone());}
            self.sessions.uri_port=Some(activation.router);
            self.sessions.peer_root=Some(activation.root);
            self.sessions.peers=Some(activation.attachment.owner.clone());
            self.peer_runtime=Some(activation.attachment.runtime.clone());
            self.sessions.peer_attachment=None;
            if let Some((owner,runtime))=old{if !Arc::ptr_eq(&owner,&activation.attachment.owner)||runtime!=activation.attachment.runtime{owner.unbind(&runtime);}}
            activation.attachment.commit();
        }
        *session.tool_owner.lock().unwrap() = tool_owner.clone();
        self.sessions.tool_owner = tool_owner;
        self.sessions.context_controller = context_controller;
        self.session = session.clone();
        self.publish_snapshot(&config);
        self.agent = agent;
        self.terminal_recovery = maintenance::TerminalRecoveryState::default();
        self.local_context_rebase = None;
        self.artifact_router.bind(session.artifacts.clone());
        self.session = session;
        self.config = config;
        self.skills = skills;
        self.drain_queues = false;
        self.restore_auto_selection().await?;
        Ok(())
    }

    async fn new_session(&mut self, parent: Option<&str>) -> Result<()> {
        self.abort().await;
        let prepared = self.fresh_actual_session()?;
        let values = prepared.as_ref().map(|prepared| prepared.values().clone());
        let native_session = prepared
            .as_ref()
            .and_then(PreparedActualSession::native_session);
        let candidate = self.session_config(true, values, native_session, Some(0)).await?;
        let mut journal = self
            .sessions
            .dir
            .as_ref()
            .map(|dir| {
                SessionJournal::create_with_parent_and_blob_directory(
                    dir,
                    &self.sessions.cwd,
                    parent,
                    &super::ara_blobs_directory(),
                )
            })
            .transpose()?;
        if let Some(journal) = journal.as_mut() {
            journal.append_model_change(&format!(
                "{}/{}",
                candidate.config.model.provider, candidate.config.model.id
            ))?;
            // An explicit new Session must survive restart even with no prompt.
            journal.materialize()?;
        }
        let header = journal
            .as_ref()
            .map(|journal| journal.header().clone())
            .unwrap_or_else(|| super::ephemeral_header(&self.sessions.cwd, parent));
        self.adopt(journal, header, Vec::new(), candidate).await?;
        Ok(())
    }

    fn candidate_context(
        &self,
        journal: &mut SessionJournal,
        values: Option<&Arc<SessionValues>>,
    ) -> Result<Vec<AgentMessage>> {
        let projector = ara_cli::agent_message_projection::AgentMessageProjector::default();
        if let Some(values) = values {
            super::bind_native_journal(&projector, journal, values)?;
        }
        ara_cli::remote_compaction::route_raw_context(
            journal,
            &projector,
            &self.config.model,
            self.sessions.provider.metadata.as_ref(),
            &self.compaction_policy.recovery_settings()?.remote,
            self.sessions.provider.route.remote_supports_images(),
        )
    }

    async fn switch_session(&mut self, path: PathBuf) -> Result<bool> {
        if self.sessions.peers.as_ref().is_some_and(|peers|peers.owns_file(&path,self.peer_runtime.as_ref())) {bail!("Session is owned by an adopted Peer runtime; release that exact owner before switching");}
        self.abort().await;
        if self.sessions.dir.is_none() {
            bail!("--no-session cannot load a disk Session; restart without --no-session");
        }
        // Join before opening, including a current lazy journal which was not
        // on disk before its accepted Run's final cancellation receipt.
        let mut journal =
            SessionJournal::open_with_blob_directory(&path, &super::ara_blobs_directory())
                .with_context(|| format!("opening session {}", path.display()))?;
        if let Some(cwd) = journal.header().get("cwd").and_then(Value::as_str)
            && lexical_absolute(std::path::Path::new(cwd))? != lexical_absolute(&self.sessions.cwd)?
        {
            // Fixed RPC supplies no onCwdChange callback: this is a cancelled
            // switch, including an inaccessible recorded cwd, not an adoption.
            return Ok(true);
        }
        let same_file = self.session.file.as_ref().is_some_and(|current| {
            std::fs::canonicalize(current)
                .ok()
                .zip(std::fs::canonicalize(&path).ok())
                .is_some_and(|(a, b)| a == b)
        });
        let file = journal.is_persistent().then(|| journal.path());
        let (values, native_session) = match self.retained_session(journal.header(), file) {
            Some(retained) => (retained.values.clone(), retained.native_session.clone()),
            None => {
                let prepared = self.fresh_actual_session()?;
                (
                    prepared.as_ref().map(|prepared| prepared.values().clone()),
                    prepared
                        .as_ref()
                        .and_then(PreparedActualSession::native_session),
                )
            }
        };
        let messages = self.candidate_context(&mut journal, values.as_ref())?;
        let unchanged = same_file
            && journal.header()["id"] == self.session.header["id"]
            && self
                .session
                .projector
                .raw_messages_equal_current(&messages, &self.agent.raw_messages().await)?;
        let candidate = self
            .session_config(!unchanged, values, native_session, Some(super::rpc_host_settings::peer_task_depth(Some(&journal))?))
            .await?;
        // The launch-selected route is currently the only available model.
        // Keep unavailable saved selections intact, as fixed switch's fallback
        // does; role/catalog/thinking restoration remains a mapped WIP gap.
        super::recover_session(&mut journal)?;
        let messages = self.candidate_context(&mut journal, candidate.values.as_ref())?;
        let header = journal.header().clone();
        self.adopt(Some(journal), header, messages, candidate)
            .await?;
        Ok(false)
    }

    async fn branch(&mut self, entry_id: &str) -> Result<String> {
        // Select raw identity before cancellation; empty/image-only ordinary
        // users are valid even though the branch picker omits their empty text.
        let (parent, text) = {
            let journal = self.session.journal.lock().await;
            let entry = journal
                .entries()
                .iter()
                .rev()
                .find(|entry| entry.id == entry_id)
                .context("Invalid entry ID for branching")?;
            if entry.kind != "message"
                || !entry
                    .observed_message_field(&["role"])?
                    .as_ref()
                    .and_then(WireValue::as_string)
                    .is_some_and(|role| role.equals_ascii("user"))
            {
                bail!("Invalid entry ID for branching");
            }
            (
                entry.parent_id.clone().filter(|parent| !parent.is_empty()),
                entry_user_text(entry)?,
            )
        };
        self.abort().await;
        let prepared = self.fresh_actual_session()?;
        let values = prepared.as_ref().map(|prepared| prepared.values().clone());
        let native_session = prepared
            .as_ref()
            .and_then(PreparedActualSession::native_session);
        let peer_depth={
            let journal=self.session.journal.lock().await;
            super::rpc_host_settings::peer_branch_task_depth(&journal,parent.as_deref())
        };
        let candidate = self.session_config(true, values, native_session, Some(peer_depth)).await?;
        let mut journal = self
            .session
            .journal
            .lock()
            .await
            .fork_at(parent.as_deref(), self.sessions.dir.as_deref())?;
        if candidate.values.is_some() {
            // A new Session imports the fork through its existing durable Native
            // boundary. Never adopt old Live graph handles into a foreign realm.
            journal.prepare_native_fork_for_realm()?;
        }
        let header = journal.header().clone();
        let messages = self.candidate_context(&mut journal, candidate.values.as_ref())?;
        self.adopt(Some(journal), header, messages, candidate)
            .await?;
        Ok(text)
    }

    // Construct the large admission future before the caller begins polling it.
    // Boxing inside an async wrapper retains construction temporaries in its poll.
    fn start(
        &mut self,
        message: Option<AgentMessage>,
        command: Command,
    ) -> Pin<Box<impl Future<Output = Result<()>> + '_>> {
        Box::pin(self.start_admitted(message, command, false, None))
    }

    fn start_after_maintenance(
        &mut self,
        command: Command,
        check_fit: bool,
        reserve_tokens: Option<f64>,
    ) -> Pin<Box<impl Future<Output = Result<()>> + '_>> {
        Box::pin(self.start_admitted(None, command, check_fit, reserve_tokens))
    }

    async fn start_admitted(
        &mut self,
        message: Option<AgentMessage>,
        command: Command,
        check_fit: bool,
        reserve_tokens: Option<f64>,
    ) -> Result<()> {
        if self.active.is_some() || self.auto.busy() || self.maintenance.is_some() {
            bail!("Agent is already running; specify streamingBehavior: steer or followUp");
        }
        if message.is_some() {
            self.auto_compaction_pending = false;
            self.header_continue = None;
            self.maintenance_continue = None;
            self.prompt_generation = self.prompt_generation.wrapping_add(1);
            self.terminal_recovery = maintenance::TerminalRecoveryState::default();
            // A new user prompt owns a fresh recovery epoch. Keep the existing
            // Provider/Responses state; request observers freeze this epoch at
            // started(), while old owned jobs retain their revoked epoch.
            let id = self.session.header["id"]
                .as_str()
                .context("Session identity is missing")?
                .to_owned();
            self.sessions.provider.admit_reset_session(
                &self.config.model,
                Some(id.clone()),
                Some(id),
            );
        }
        self.fallback.clear_ready();
        let drain_queues = self.drain_queues;
        let session = self.session.clone();
        let generation = self.prompt_generation;
        let restore_pending = self.fallback_restore_pending();
        let needs_quota = restore_pending || self.retry_policy.usage_aware_fallback();
        let restored = if restore_pending {
            let guard = quota::QuotaControl::begin(&self.quota_control, &self.connection);
            tokio::select! {
                biased;
                _ = guard.cancel.cancelled() => return Err(ara_cli::handoff::cancelled()),
                result = self.restore_fallback_primary(&guard.cancel) => result?,
            }
        } else {
            false
        };
        let pending = match &message {
            Some(input) => vec![input.clone()],
            None => {
                let mut queued = self.agent.peek_steering_raw_messages();
                if self.agent.steering_mode() == QueueMode::OneAtATime {
                    queued.truncate(1);
                }
                queued
            }
        };
        let message = if let Some(input) = message {
            // Native ordinary prompt: restore, reserve ready, then full-message
            // maintenance. A later route replacement invalidates that ready.
            if self.retry_policy.usage_aware_fallback() {
                let live = self.agent.raw_messages().await;
                self.initial_usage_preflight(&live).await?;
            }
            if self.auto.enabled()
                && (input.json()?["role"] == "user" || input_skill(&input)?.is_some()
                    || self.child_turn.as_ref().is_some_and(|turn|!turn.is_continuation())&&input.json()?["customType"]=="child-assignment")
            {
                self.begin_auto_start(input, command, restored, drain_queues)?;
                return Ok(());
            }
            let ready = self.fallback.take_ready();
            let maintained = if restored {
                self.maybe_auto_compact_for_restore(&pending).await
            } else {
                self.maybe_auto_compact(&pending).await
            };
            if !maintained {
                return Err(ara_cli::handoff::cancelled());
            }
            self.restore_usage_ready(ready);
            Some(input)
        } else {
            // Native scheduled continuation: restore, maintain a restored
            // smaller window, then reserve. Keep the originating retry saga
            // out of the idle maintenance abort and refresh its expected view.
            if restored {
                let retry = self.retry.take();
                let ready = self.maybe_auto_compact_for_restore(&pending).await;
                self.retry = retry;
                if ready && let Some(retry) = &mut self.retry {
                    let messages = self.agent.raw_messages().await;
                    retry.expected_messages = if retry.native_mode {
                        Vec::new()
                    } else {
                        self.session.projector.current_body_receipts(&messages)?
                    };
                    if retry.native_mode {
                        retry.native_context = Some(NativeRetryContextReceipt::capture(
                            &messages,
                            self.session
                                .values
                                .clone()
                                .context("Native restored retry values missing")?,
                        )?);
                    }
                }
                if !ready {
                    return Err(ara_cli::handoff::cancelled());
                }
            } else if self.agent.has_queued_messages() && !self.maybe_auto_compact(&pending).await {
                return Err(ara_cli::handoff::cancelled());
            }
            if self.retry_policy.usage_aware_fallback() {
                let live = self.agent.raw_messages().await;
                self.initial_usage_preflight(&live).await?;
            }
            None
        };
        if !Arc::ptr_eq(&session, &self.session)
            || generation != self.prompt_generation
            || (needs_quota && self.quota_control.lock().unwrap().stop_requested())
            || self.connection.is_cancelled()
        {
            self.fallback.clear_ready();
            return Err(ara_cli::handoff::cancelled());
        }
        if check_fit && !self.recovery_fits(reserve_tokens, None, &[]).await? {
            self.fallback.clear_ready();
            bail!("Context changed after compaction and no longer fits the model.");
        }
        self.drain_queues = drain_queues;
        self.dispatch(message, command)
    }

    fn dispatch(&mut self, message: Option<AgentMessage>, command: Command) -> Result<()> {
        let outcome=self.dispatch_inner(message,command);
        if let Err(error)=&outcome{if let Some(turn)=self.child_turn.take(){turn.fail(error.to_string());}}
        outcome
    }
    fn dispatch_inner(&mut self, message: Option<AgentMessage>, command: Command) -> Result<()> {
        // The reader can accept Abort while an inline side request returns,
        // before the serial command queue gets its next turn.
        if self.handoff_control.lock().unwrap().abort_requested() {
            return Err(ara_cli::handoff::cancelled());
        }
        if self.session.persistence_error.lock().unwrap().is_some() {
            bail!("session persistence failed; restart from the journal before continuing");
        }
        if self.active.is_some() || self.maintenance.is_some() {
            bail!("Agent is already running; specify streamingBehavior: steer or followUp");
        }
        if let Some(owner)=self.sessions.tool_owner.native.as_ref().and_then(|n|n.child_contract.clone()) {
            let turn=self.child_turn.clone().unwrap_or_else(||super::child_host::ChildTurn::new(owner.published_snapshot().generation,false));
            turn.release_recovery_lease();
            let kind=if turn.pooled {ara_cli::child_contract::DispatchKind::Pooled}else{ara_cli::child_contract::DispatchKind::Ordinary};
            let lease=owner.try_claim_dispatch(kind,false)?;
            if lease.generation()!=turn.expected_generation {bail!("Child contract changed before Native dispatch; no prompt replayed");}
            let bundle=self.sessions.tool_owner.native.as_ref().and_then(|n|n.child_yield.clone()).context("Child has no exact Native yield observer")?;
            turn.admit_native(&bundle,lease)?;
            self.child_turn=Some(turn);
        }
        self.rebind_context_thinking();
        let header_guard_enabled = loop_guard::guard_enabled(self.loop_guard_settings)
            && self.loop_guard_settings.tool_call_reminder
            && ara_cli::model_route::resolved_loop_guard_model_class(&self.config.model).map_err(
                |error| anyhow::anyhow!("resolving native loop guard model class: {error:?}"),
            )? == "gemini";
        let cancel = self.connection.child_token();
        let mut config = self.config.clone();
        if self.child_turn.as_ref().is_some_and(|turn|turn.forced_final()){config.tool_choice=Some(ara_ai::ToolChoice::Function("yield".into()));}
        config.deadline = self
            .max_time
            .map(|seconds| Instant::now() + Duration::from_secs_f64(seconds.max(0.0)));
        let tool_loop_settings = match &self.tool_loop_settings_dir {
            Some(directory) => ToolLoopGuardSettings::load(directory)?,
            None => ToolLoopGuardSettings::default(),
        };
        self.unexpected_stop_mode = match &self.tool_loop_settings_dir {
            Some(directory) => UnexpectedStopMode::load(directory)?,
            None => UnexpectedStopMode::default(),
        };
        let edit_guard = super::make_streaming_edit_guard(
            &self.sessions.tool_owner,
            self.tool_loop_settings_dir.as_deref(),
        )?;
        let native = self
            .session
            .native_session
            .as_ref()
            .map(|prepared| ara_cli::native_run::NativeRun::begin(prepared.clone(), cancel.clone()))
            .transpose()?;
        let native_lease = native.as_ref().map(ara_cli::native_run::NativeRun::lease);
        let plan_epoch = self
            .sessions
            .acp_plan
            .as_ref()
            .map(|plan| plan.reference_epoch());
        let plan_input = if message.is_some() {
            match &self.sessions.acp_plan {
                Some(plan) => {
                    let names = config
                        .tools
                        .iter()
                        .map(|tool| tool.definition().name)
                        .collect::<Vec<_>>();
                    let mode = if self.sessions.tool_owner.edit_policy.tool_mode
                        == ara_edit::EditMode::Hashline
                    {
                        "hashline"
                    } else {
                        "replace"
                    };
                    plan.prompt_context(&names, mode).map_err(anyhow::Error::msg)?
                        .map(|content| self.session.projector.new_custom_input(json!({
                            "role":"custom", "customType":if plan.current_mode() == "plan" { "plan-mode-context" } else { "plan-mode-reference" },
                            "content":content,"display":false,"attribution":"agent","timestamp":chrono::Utc::now().timestamp_millis()
                        }))).transpose()?
                }
                None => None,
            }
        } else {
            None
        };
        let reference_plan = self
            .sessions
            .acp_plan
            .clone()
            .filter(|plan| plan.current_mode() != "plan")
            .zip(plan_epoch)
            .zip(plan_input.as_ref().map(AgentMessage::weak_identity));
        config.hooks = Arc::new(RpcHooks {
            base: config.hooks,
            snapshot: self.snapshot.clone(),
            forced_child_yield: self.child_turn.as_ref().is_some_and(|turn| turn.forced_final()),
            publication: self.snapshot_publication.clone(),
            projector: self.session.projector.clone(),
            session: self.session.clone(),
            admission: self.retry_policy.usage_aware_fallback().then(|| {
                fallback::AdmissionBinding {
                    tx: self.admission_tx.clone(),
                    session: self.session.clone(),
                    generation: self.prompt_generation,
                    route_generation: self.route_generation.clone(),
                }
            }),
        });
        config.hooks = Arc::new(ToolLoopHooks {
            base: config.hooks,
            native_session: self.session.native_session.clone(),
            state: self.tool_loop.clone(),
            settings: tool_loop_settings,
            output: self.output.clone(),
            session_id: self.session.header["id"].clone(),
            run_cancel: cancel.clone(),
        });
        let sink = Arc::new(RunSink {
            session: self.session.clone(),
            observed_usage: self.sessions.provider.shared_account.clone(),
            output: self.output.clone(),
            cancel: cancel.clone(),
            connection: self.connection.clone(),
            terminal: Mutex::new(None),
            messages: Mutex::new(Vec::new()),
            entries: Mutex::new(Vec::new()),
            header_guard: Mutex::new(GeminiHeaderGuard::new(
                header_guard_enabled,
                self.prompt_generation,
            )),
        });
        if let Some(context) = &self.sessions.tool_owner.context {
            super::host_edit_post_apply::HostEditPostApply::install(
                context,
                &self.sessions.provider,
                self.sessions.cwd.clone(),
                self.sessions.args.models_config.clone(),
                self.session.header["id"].as_str().unwrap_or("").to_owned(),
                super::host_edit_post_apply::active_model_for_hooks(&config.model, &config.hooks),
                edit_repair_usage_factory(
                    self.session.clone(),
                    self.sessions.provider.shared_account.clone(),
                    self.connection.clone(),
                ),
            );
        }
        let agent = self.agent.clone();
        let run_cancel = cancel.clone();
        let run_sink = Arc::new(ara_cli::streaming_edit_sink::StreamingEditSink::new(
            sink.clone(),
            edit_guard,
            self.session.projector.actual_values(),
            cancel.clone(),
        ));
        if let(Some(peers),Some(runtime))=(&self.sessions.peers,&self.peer_runtime){
            let admitted=if let Some(wake)=&self.peer_wake{peers.coordinator.registry.begin_wake(runtime,wake.request.job.turn)}else{peers.coordinator.registry.begin_ordinary_turn(runtime)};
            anyhow::ensure!(admitted,"Peer original turn lost runtime/root admission before Native dispatch");
        }
        // The serial Host owns the slot before it can accept another command.
        // ACK was enqueued by the caller before this task can emit AgentStart.
        let task = tokio::spawn(async move {
            // Abort/new/switch may cancel the accepted dispatch before the
            // spawned task enters Agent. Do not append an uninvoked prompt.
            if run_cancel.is_cancelled() {
                return Ok(None);
            }
            let agent_future = async move {
                match message {
                    Some(message) => {
                        let mut inputs = plan_input.into_iter().collect::<Vec<_>>();
                        inputs.push(message);
                        let result = agent
                            .prompt_raw_with_config(inputs, config, run_cancel, run_sink)
                            .await;
                        // Mark only after a reference actually entered current
                        // Agent history. A cancelled dispatch can be retried.
                        if let Some(((plan, epoch), identity)) = reference_plan {
                            if agent
                                .raw_messages()
                                .await
                                .iter()
                                .any(|message| message.weak_identity() == identity)
                            {
                                plan.mark_reference_sent_at(epoch);
                            }
                        }
                        result
                    }
                    None => {
                        agent
                            .continue_run_with_config(config, run_cancel, run_sink)
                            .await
                    }
                }
                .map(Some)
            };
            match native {
                Some(native) => Box::pin(native.drive(agent_future))
                    .await
                    .map_err(|error| {
                        AgentError::RawContext(RawContextError(format!(
                            "Native Run driver failed: {error}"
                        )))
                    })?,
                None => agent_future.await,
            }
        });
        self.active = Some(ActiveRun {
            child_turn:self.child_turn.take(),
            native: native_lease,
            cancel,
            task,
            command,
            sink,
        });
        Ok(())
    }

    async fn completed(
        &mut self,
        active: ActiveRun,
        result: std::result::Result<
            std::result::Result<Option<RunReport>, AgentError>,
            tokio::task::JoinError,
        >,
    ) {
        // Join the independent effect owner before any projection/terminal/recovery
        // branch, including a panicked or never-entered spawned graph driver.
        let mut result = result;
        let mut native_settlement_error = None;
        if let Some(native) = &active.native {
            native.revoke_graph();
            if let Err(error) = Box::pin(native.settle()).await {
                if result.as_ref().is_ok_and(|outcome| outcome.is_ok()) {
                    result = Ok(Err(AgentError::RawContext(RawContextError(
                        error.to_string(),
                    ))));
                } else {
                    native_settlement_error = Some(error.to_string());
                }
            }
        }
        let child_known_model_failure=!active.cancel.is_cancelled()&&result.as_ref().is_ok_and(|outcome|outcome.as_ref().is_ok_and(|report|report.as_ref().is_some_and(|report|report.end==RunEnd::Error)));
        let child_clean=result.as_ref().is_ok_and(|outcome|outcome.as_ref().is_ok_and(|report|report.as_ref().is_some_and(|report|matches!(report.end,RunEnd::Completed|RunEnd::TerminalToolResult))));
        let child_cancelled=active.cancel.is_cancelled()||result.as_ref().is_ok_and(|outcome|outcome.as_ref().is_ok_and(|report|report.as_ref().is_some_and(|report|report.end==RunEnd::Aborted)));
        if let Some(wake)=&mut self.peer_wake {
            let observed=result.as_ref().ok().and_then(|result|result.as_ref().ok()).and_then(Option::as_ref).map(|report|super::child_host::report_update(report,&active.sink.session.projector)).transpose();
            wake.turn=active.child_turn.clone();
            wake.outcome=Some(match observed {
                Ok(update)=>{let answer=match update{Some(super::child_host::ReportUpdate::Report(text))=>text.to_utf8().ok(),_=>None};ara_cli::host_peers::WakeOutcome{accepted:child_clean,response_at:answer.as_ref().map(|_|ara_cli::host_peers::now_ms()),answer,output_path:None,error:(!child_clean&&!child_cancelled).then(||"Peer Native turn failed".into()),cancelled:child_cancelled,accepted_yield:false}},
                Err(error)=>ara_cli::host_peers::WakeOutcome::failed(error.to_string()),
            });
        }
        let child_text=if active.child_turn.is_some(){
            match result.as_ref().ok().and_then(|r|r.as_ref().ok()).and_then(Option::as_ref)
                .map(|report|super::child_host::report_update(report,&active.sink.session.projector)).transpose(){
                Ok(update)=>update.unwrap_or(super::child_host::ReportUpdate::NoUpdate),Err(error)=>{if let Some(turn)=&active.child_turn{turn.fail(error.to_string());}return;}
            }
        }else{super::child_host::ReportUpdate::NoUpdate};
        let mut child_completion=super::child_host::ChildCompletionGuard::new(active.child_turn.clone());
        if let Some(turn)=&active.child_turn{self.child_turn=Some(turn.clone());}
        self.fallback.clear_ready();
        let (terminal_assistant, terminal_actual) = if let Ok(Ok(Some(report))) = &result {
            match completed_assistant(report, &active.sink.session.projector) {
                Ok(message) => message,
                Err(error) => {
                    active.sink.persistence_failed(&error);
                    self.output
                        .response(&active.command, None, Some(error.to_string()));
                    self.drain_queues = false;
                    return;
                }
            }
        } else {
            (None, None)
        };
        // Both callers joined the task and removed the active slot. Publish
        // idle only after the Agent's running guard and transcript lock release.
        if let Some(mut terminal) = active.sink.terminal.lock().unwrap().take() {
            if let Ok(Ok(Some(report))) = &result {
                // Additive Host diagnostic from this settled Run; Core's
                // ordinary native agent_end event retains its original shape.
                match &report.stop_cause {
                    Some(ara_agent::RunStopCause::TerminalToolResult { tool_call_id }) => {
                        terminal.insert(
                            "stopCause",
                            wire(json!({"type":"terminalToolResult","toolCallId":tool_call_id})),
                        );
                    }
                    Some(ara_agent::RunStopCause::PreModelStop { reason }) => {
                        terminal.insert(
                            "stopCause",
                            wire(json!({"type":"preModelStop","reason":reason})),
                        );
                    }
                    _ => {}
                }
            }
            self.output.frame_wire(terminal);
        }
        let persistence_error = active
            .sink
            .session
            .persistence_error
            .lock()
            .unwrap()
            .clone();
        // Retain one owned CURRENT Assistant view across the awaited quota and
        // maintenance callbacks. It never replaces the canonical raw record.
        // Request feedback already has its own owner/token. Join its matched
        // proof before persistence and maintenance early exits, even when the
        // failed turn is ineligible for reset or replay.
        let quota_outcome = if let Ok(Ok(Some(report))) = &result {
            let outcome = if let Some(message) = terminal_actual.as_ref() {
                let replay = if persistence_error.is_none()
                    && matches!(report.end, RunEnd::Error | RunEnd::Aborted)
                {
                    async {
                        let terminal = RecoveryTerminal::native(message.clone(), self)?;
                        terminal.quota_replay_admission(self, &active).await
                    }
                    .await
                } else {
                    Ok(false)
                };
                match replay {
                    Err(error) => Err(error),
                    Ok(replay) => {
                        Box::pin(self.session_quota_outcome_actual(
                            &active,
                            message,
                            report.end,
                            persistence_error.is_none(),
                            replay,
                        ))
                        .await
                    }
                }
            } else if let Some(message) = terminal_assistant.as_ref() {
                Box::pin(self.session_quota_outcome(
                    &active,
                    message,
                    report.end,
                    persistence_error.is_none(),
                ))
                .await
            } else {
                Ok(None)
            };
            match outcome {
                Ok(outcome) => outcome,
                Err(error) => {
                    if error.downcast_ref::<ara_agent::RawContextError>().is_some()
                        || error
                            .downcast_ref::<ara_agent::ActualProviderError>()
                            .is_some()
                        || error
                            .downcast_ref::<ara_rpc::value_heap::ValueError>()
                            .is_some()
                    {
                        active.sink.persistence_failed(&error);
                        self.output
                            .response(&active.command, None, Some(error.to_string()));
                        self.drain_queues = false;
                        return;
                    }
                    self.output.frame(
                        json!({"type":"notice","level":"warning","source":"quota-recovery",
                        "message":super::sanitize_text(&error.to_string())}),
                    );
                    Some(super::session_quota_recovery::SessionQuotaOutcome {
                        recorded: false,
                        switched: false,
                        wait_ms: None,
                        sibling_retry_at_ms: None,
                    })
                }
            }
        } else {
            None
        };
        if persistence_error.is_none()
            && let Ok(Ok(Some(report))) = &result
        {
            let header_hit = active.sink.header_guard.lock().unwrap().hit();
            if report.end == RunEnd::Aborted
                && let Some(hit) = header_hit
            {
                match self.begin_header_continue(&active, hit).await {
                    Ok(true) => {child_completion.defer();return;},
                    Ok(false) => {}
                    Err(error) => {
                        active.sink.persistence_failed(error);
                    }
                }
            }
            let recovery_terminal = match terminal_actual.as_ref() {
                Some(message) => match RecoveryTerminal::native(message.clone(), self) {
                    Ok(terminal) => Some(terminal),
                    Err(error) => {
                        active.sink.persistence_failed(error);
                        return;
                    }
                },
                None => terminal_assistant.clone().map(RecoveryTerminal::Legacy),
            };
            let assistant = recovery_terminal.as_ref();
            if report.end == RunEnd::Completed
                && let Some(message) = assistant
            {
                if let Err(error) = self.note_served_model_terminal(message).await {
                    active.sink.persistence_failed(error);
                    return;
                }
            }
            let mut maintenance_blocked = false;
            if matches!(report.end, RunEnd::Error | RunEnd::Completed)
                && let Some(message) = assistant
            {
                match Box::pin(self.begin_empty_stop_recovery_terminal(&active, message)).await {
                    Ok(outcome) if outcome.continuation_scheduled => {child_completion.defer();return;},
                    Ok(outcome) if outcome.automatic_continuation_blocked => {
                        self.auto_compaction_pending = false;
                        self.drain_queues = self.agent.has_queued_messages();
                        self.output.response(
                            &active.command,
                            None,
                            Some(
                                "Assistant returned no final output after the recovery cap".into(),
                            ),
                        );
                        return;
                    }
                    Ok(_) => {}
                    Err(error) => {
                        active.sink.persistence_failed(error);
                        return;
                    }
                }
            }
            if matches!(report.end, RunEnd::Error | RunEnd::Completed)
                && let Some(message) = assistant
            {
                match Box::pin(self.begin_unexpected_stop_recovery_terminal(&active, message)).await
                {
                    Ok(outcome) if outcome.continuation_scheduled => {child_completion.defer();return;},
                    Ok(_) => {}
                    Err(error) => {
                        active.sink.persistence_failed(error);
                        return;
                    }
                }
            }
            if matches!(report.end, RunEnd::Error | RunEnd::Completed)
                && let Some(message) = assistant
            {
                // Serial threshold compaction also joins completed(). Keep the
                // maintenance future off the Windows main thread's small stack.
                match Box::pin(self.begin_overflow_recovery_terminal(&active, message)).await {
                    Ok(outcome) => {
                        if outcome.continuation_scheduled || outcome.deferred_handoff {
                            child_completion.defer();return;
                        }
                        maintenance_blocked =
                            outcome.automatic_continuation_blocked || outcome.history_rewritten;
                        if maintenance_blocked {
                            self.finish_retry(
                                None,
                                Some("Context overflow recovery stopped".into()),
                                false,
                            )
                            .await;
                            self.auto_compaction_pending = false;
                            self.drain_queues = self.agent.has_queued_messages();
                            let error = match message.error_message() {
                                Ok(error) => error
                                    .unwrap_or_else(|| "Context overflow recovery stopped".into()),
                                Err(error) => {
                                    active.sink.persistence_failed(error);
                                    return;
                                }
                            };
                            self.output.response(&active.command, None, Some(error));
                            return;
                        }
                    }
                    Err(error) => {
                        active.sink.persistence_failed(error);
                        return;
                    }
                }
            }
            if matches!(report.end, RunEnd::Error | RunEnd::Aborted)
                && !maintenance_blocked
                && let Some(message) = assistant
            {
                match self
                    .begin_retry_terminal(&active, message, quota_outcome.as_ref())
                    .await
                {
                    Ok(true) => {child_completion.defer();return;},
                    Ok(false) => {}
                    Err(error) => {
                        self.finish_retry(
                            None,
                            Some(format!("Retry continuation failed locally: {error}")),
                            false,
                        )
                        .await;
                    }
                }
            } else if matches!(report.end, RunEnd::Completed | RunEnd::TerminalToolResult) {
                self.finish_retry_terminal(assistant, None, false).await;
            }
        }
        let error = match result {
            Ok(Ok(None)) => {
                self.output.prompt_skipped(&active.command);
                None
            }
            Ok(Ok(Some(report))) => match report.end {
                RunEnd::Completed => {
                    self.auto_compaction_pending = true;
                    None
                }
                RunEnd::TerminalToolResult => None,
                RunEnd::PreModelStop => Some(match &report.stop_cause {
                    Some(ara_agent::RunStopCause::PreModelStop {
                        reason: Some(reason),
                    }) => {
                        format!("Stopped before the model call: {reason}")
                    }
                    _ => "Stopped before the model call".into(),
                }),
                RunEnd::Aborted => Some("Request was aborted".into()),
                RunEnd::Deadline => Some("Deadline exceeded".into()),
                RunEnd::ModelCallBudget => Some("Model call limit reached".into()),
                RunEnd::Error => {
                    if let Some(message) = terminal_actual.as_ref() {
                        match actual_terminal_error(message, &active.sink.session.projector) {
                            Ok(error) => error.or_else(|| Some("Agent run failed".into())),
                            Err(error) => {
                                active.sink.persistence_failed(&error);
                                Some(error.to_string())
                            }
                        }
                    } else {
                        terminal_assistant
                            .as_ref()
                            .and_then(|message| message.error_message.clone())
                            .or_else(|| Some("Agent run failed".into()))
                    }
                }
            },
            Ok(Err(error)) => Some(error.to_string()),
            Err(error) => Some(format!("Run failed; tool effects may be unknown: {error}")),
        };
        let error = persistence_error
            .map(|error| format!("Session persistence failed ({error}); restart from the journal"))
            .or(error);
        let error = match (error, native_settlement_error) {
            (Some(primary), Some(settlement)) => Some(format!("{primary}; {settlement}")),
            (None, Some(settlement)) => Some(settlement),
            (error, None) => error,
        };
        let known_model_error=if child_known_model_failure{error.clone()}else{None};
        let child_error=self.session.persistence_error.lock().unwrap().clone();
        if let Some(error) = error {
            if let Some(wake)=&mut self.peer_wake{if let Some(outcome)=&mut wake.outcome{outcome.error=Some(error.clone());outcome.accepted=false;}}
            self.finish_retry(None, Some(error.clone()), false).await;
            self.auto_compaction_pending = false;
            self.drain_queues = false;
            self.output.response(&active.command, None, Some(error));
        }
        self.child_turn=None;
        let child_error=child_error.or_else(||self.session.persistence_error.lock().unwrap().clone());
        if let Some(error)=child_error{if let Some(turn)=&active.child_turn{turn.fail(error);}child_completion.disarm();}
        else{
            child_completion.finish(child_clean,child_cancelled,child_text);
            if let (Some(turn),Some(error))=(&active.child_turn,known_model_error){turn.note_known_model_error(error);}
        }
        if let Some(native)=&self.sessions.tool_owner.native{native.peer_ingress.release_unappended();native.wait_session.release_unappended();}
        if self.peer_wake.is_none(){if let(Some(peers),Some(runtime))=(&self.sessions.peers,&self.peer_runtime){peers.coordinator.registry.set_status(&runtime.peer,ara_cli::host_peers::PeerStatus::Idle);}}

    }

    async fn abort(&mut self) {
        self.cancel_auto_start();
        self.fallback.clear_ready();
        self.header_continue = None;
        // Keep the owned maintenance settlement out of the established
        // new/switch/compact future layouts on the Windows main thread.
        Box::pin(self.abort_maintenance()).await;
        self.finish_retry(None, Some("Retry cancelled".into()), false)
            .await;
        self.auto_compaction_pending = false;
        self.drain_queues = false;
        if let Some(mut active) = self.active.take() {
            // NativeRun revokes after entered methods settle and publish their
            // original receipts. Abort only its shared signal at this boundary.
            active.sink.header_guard.lock().unwrap().revoke();
            // Covers the interval before Agent::enter installs its own token.
            active.cancel.cancel();
            self.agent.abort();
            let result = (&mut active.task).await;
            // Recovery adds owned async state; keep settlement out of the
            // new/switch/abort callers on the Windows default main stack.
            Box::pin(self.completed(active, result)).await;
        }
        self.flush_pending_bash().await;
        // An already-settling Run can complete successfully after cancellation.
        // An explicit abort must still suppress new automatic maintenance.
        self.auto_compaction_pending = false;
    }

    fn late_idle_allowed(&self) -> bool {
        !self.input_closed
            && self.active.is_none()
            && !self.auto.busy()
            && self.retry.is_none()
            && self.header_continue.is_none()
            && self.maintenance.is_none()
            && self.maintenance_continue.is_none()
            && self.drain_queues
            && self.session.persistence_error.lock().unwrap().is_none()
            && !self.connection.is_cancelled()
    }
    async fn reconcile_late_diagnostics(&mut self) {
        if !self.late_idle_allowed() {
            return;
        }
        let Some(owner) = self.session.edit_session() else {
            return;
        };
        let message = match owner.idle_message(
            self.session.projector.clone(),
            self.session.native_session.clone(),
        ) {
            Ok(Some(message)) => message,
            Ok(None) => return,
            Err(error) => {
                self.output.frame(json!({"type":"notice","level":"error","source":"lsp_late_diagnostics","message":error.to_string()}));
                return;
            }
        };
        // Same Session ordinary prompt admission, after completed() joined the
        // Run/native effects. No RPC command ACK precedes journal MessageEnd.
        let command = Command::new(wire(json!({"type":"prompt"})));
        if let Err(error) = self.start(Some(message), command).await {
            self.output.frame(
                json!({"type":"notice","level":"error","source":"lsp_late_diagnostics","message":error.to_string()}),
            );
        }
    }

    async fn reconcile_queues(&mut self) {
        if self.active.is_none()
            && !self.auto.busy()
            && self.retry.is_none()
            && self.header_continue.is_none()
            && self.maintenance.is_none()
            && self.maintenance_continue.is_none()
            && self.drain_queues
            && self.agent.has_queued_messages()
            && !self.connection.is_cancelled()
        {
            let command = Command::new(wire(json!({"type":"prompt"})));
            if let Err(error) = self.start(None, command).await {
                self.drain_queues = false;
                self.output
                    .frame(json!({"type":"response","command":"prompt","success":false,"error":error.to_string()}));
            }
        }
        self.reconcile_late_diagnostics().await;
    }

    // The admission path and fixture both ask this actual Host policy owner.
    fn compaction_required(&self,tokens:usize,threshold:usize,restored:bool)->bool {
        self.compaction_policy.enabled() && (restored || threshold != 0) && tokens > threshold
    }

    async fn child_compaction_threshold(&self, threshold: usize) -> usize {
        if let Some(pair)=self.session.journal.lock().await.latest_child_init().ok().flatten().and_then(|init|init.compaction_threshold) {
            match self.config.model.context_window.filter(|w|w.is_finite()&&*w>0.0) {
                Some(window)=>{
                    let reserve=self.compaction_policy.recovery_settings().ok().and_then(|s|s.reserve_tokens);
                    let budget=maintenance::retry_context_capacity(Some(window),reserve).unwrap_or(window);
                    super::rpc_host_settings::child_compaction_threshold(&pair,window.floor()as usize,budget)
                }
                None=>threshold,
            }
        }else{threshold}
    }

    async fn state(&self) -> Value {
        let (steering, follow_up) = self.agent.queued_counts();
        let model = &self.config.model;
        let catalog_id = self
            .sessions
            .provider
            .metadata
            .as_ref()
            .and_then(|metadata| metadata["catalogModelId"].as_str())
            .unwrap_or(&model.id);
        let mut state = json!({
            "model":{"id":catalog_id,"provider":model.provider,"api":model.api,"baseUrl":model.base_url,"reasoning":model.reasoning},
            "isStreaming":self.active.is_some() || self.auto.busy(),"isCompacting":self.is_compacting,
            "steeringMode":mode_name(self.agent.steering_mode()),"followUpMode":mode_name(self.agent.follow_up_mode()),
            "interruptMode":interrupt_mode_name(self.agent.interrupt_mode()), "sessionId":self.session.header["id"],
            "autoCompactionEnabled":self.compaction_policy.enabled(),"fastModeEnabled":false,"fastModeActive":false,
            "autoRetryEnabled":self.retry_policy.enabled(),"isRetrying":self.retry.as_ref().is_some_and(|retry| retry.visible),
            "tokensPerSecond":null,"messageCount":self.session.messages.lock().unwrap().len(),
            "queuedMessageCount":steering+follow_up,"todoPhases":[],"systemPrompt":self.config.system_prompt,
            "dumpTools":self.config.tools.iter().map(|tool| tool.definition()).collect::<Vec<_>>()
        });
        #[cfg(feature="test-fixture")]
        { state["fixturePeerActivation"]=json!({"owner":self.sessions.peers.as_ref().map(|owner|Arc::as_ptr(owner) as usize),"runtime":self.peer_runtime,"root":self.sessions.peer_root}); }
        #[cfg(feature="test-fixture")]
        { state["fixtureAgentScope"] = json!({
            "session":self.session.agent_context.as_ref().map(|context|context.receipt()),
            "factory":self.sessions.agent_context.as_ref().map(|context|context.receipt()),
            "provider":self.sessions.provider.agent_context.as_ref().map(|context|context.receipt()),
            "accountOwner":self.sessions.provider.shared_account.as_ref().map(|account|Arc::as_ptr(account) as usize),
            "model":self.config.model.id,"providerId":self.config.model.provider,
            "registry":self.sessions.provider.registry.as_ref().map(|registry|registry.session_scope_receipt()),
            "resolver":self.sessions.provider.route.auth_for_session(self.session.header["id"].as_str()).session_scope_receipt(),
            "childThreshold":self.child_compaction_threshold(self.sessions.args.compact_threshold).await,
            "compactionDecisions":{
                "130000":self.compaction_required(130000,self.child_compaction_threshold(self.sessions.args.compact_threshold).await,false),
                "144001":self.compaction_required(144001,self.child_compaction_threshold(self.sessions.args.compact_threshold).await,false)
            }
        }); }
        if let Some(file) = &self.session.file {
            state["sessionFile"] = json!(file);
        }
        if let Some(name) = &*self.session.name.lock().unwrap() {
            state["sessionName"] = json!(name);
        }
        let served = self
            .fallback
            .serving(&self.session.header, self.serving_selector().await);
        state["servingModel"] = json!({"selector":served.selector,"isFallback":served.is_fallback});
        let journal = self.session.journal.lock().await;
        state["thinkingLevel"] = json!(journal.current_thinking_level());
        state["configuredThinkingLevel"] = json!(journal.current_configured_thinking_level());
        // Initial/resumed runtime selection can differ from saved receipts.
        if let Some(request) = self.auto.current_request() {
            let effective = if request.disable_reasoning {
                Some("off")
            } else {
                request.effort.map(ara_ai::ReasoningEffort::as_str)
            };
            state["thinkingLevel"] = json!(effective);
            state["configuredThinkingLevel"] = if self.auto.enabled() {
                json!("auto")
            } else {
                json!(effective)
            };
        }
        state
    }

    // Construct the command future outside the serial transport poll frame.
    fn handle(&mut self, command: Command) -> Pin<Box<impl Future<Output = ()> + '_>> {
        Box::pin(self.handle_owned(command))
    }

    async fn handle_owned(&mut self, command: Command) {
        let result = self.execute(&command).await;
        if let Err(error) = result {
            self.output
                .response(&command, None, Some(error.to_string()));
        }
        self.reconcile_queues().await;
    }

    async fn execute(&mut self, command: &Command) -> Result<()> {
        if auto::StartControl::pauses_start(command) {
            self.auto.control.lock().unwrap().consume();
        }
        if handoff::HandoffControl::stops_handoff(command) {
            self.handoff_control.lock().unwrap().consume_stop();
        }
        if quota::QuotaControl::stops_quota(command) {
            self.quota_control.lock().unwrap().consume_stop();
        }
        if models::stops_selection(command) {
            self.model_selection_control.lock().unwrap().consume_stop();
        }
        match command.kind.as_str() {
            "set_model" => {
                let model = self
                    .set_rpc_model(&command.string("provider")?, &command.string("modelId")?)
                    .await?;
                self.output.response_wire(command, Some(model), None, None);
            }
            "cycle_model" => {
                let result = self.cycle_rpc_model().await?;
                self.output.response_wire(command, Some(result), None, None);
            }
            "get_available_models" => {
                let models = self.rpc_available_models().await?;
                let mut data = wire(json!({}));
                data.insert("models", WireValue::Array(models));
                self.output.response_wire(command, Some(data), None, None);
            }
            "set_thinking_level" => {
                self.set_thinking_level(&command.string("level")?).await?;
                self.output.response(command, None, None);
            }
            "get_append_only_context"
            | "set_append_only_context"
            | "reload_append_only_context" => {
                let controller = self
                    .sessions
                    .context_controller
                    .as_ref()
                    .context("Session context settings are unavailable")?;
                if command.kind == "set_append_only_context" {
                    let mode = ara_cli::append_only_context_mode::AppendOnlyContextMode::parse(
                        &command.string("mode")?,
                    )
                    .map_err(anyhow::Error::msg)?;
                    controller
                        .settings()
                        .set(mode)
                        .map_err(anyhow::Error::msg)?;
                } else if command.kind == "reload_append_only_context" {
                    controller.settings().reload().map_err(anyhow::Error::msg)?;
                }
                self.output.response(
                    command,
                    Some(json!({"mode":controller.settings().get().as_str(),
                    "enabled":controller.binding().append_only_context.is_some()})),
                    None,
                );
            }
            "set_auto_retry" => {
                let enabled = match command.frame.get("enabled") {
                    Some(WireValue::Bool(enabled)) => *enabled,
                    _ => bail!("set_auto_retry requires boolean enabled"),
                };
                self.retry_policy.set_enabled(enabled)?;
                self.output.response(command, None, None);
            }
            "abort_retry" => {
                self.abort_retry().await;
                self.output.response(command, None, None);
            }
            "compact" => {
                let focus = command
                    .frame
                    .get("customInstructions")
                    .map(|_| command.string("customInstructions"))
                    .transpose()?;
                let result = self.compact(focus.as_deref()).await?;
                self.output.response_wire(command, Some(result), None, None);
            }
            "set_auto_compaction" => {
                let enabled = match command.frame.get("enabled") {
                    Some(WireValue::Bool(enabled)) => *enabled,
                    _ => bail!("set_auto_compaction requires boolean enabled"),
                };
                self.compaction_policy.set_enabled(enabled)?;
                self.auto_compaction_checked = None;
                self.output.response(command, None, None);
            }
            "bash" => self.bash_dispatcher.dispatch(Command {
                id: command.id.clone(),
                kind: command.kind.clone(),
                frame: command.frame.clone(),
            }),
            "abort_bash" => {
                self.bash_dispatcher.abort();
                self.output.response(command, None, None);
            }
            "set_host_tools" => {
                let definitions = normalize_host_tool_definitions(
                    command
                        .frame
                        .get("tools")
                        .context("set_host_tools requires tools")?,
                )
                .map_err(anyhow::Error::msg)?;
                let names = self.set_host_tools(definitions).await?;
                self.output
                    .response(command, Some(json!({"toolNames":names})), None);
            }
            "set_host_uri_schemes" => {
                let schemes = self
                    .uri_bridge
                    .set_schemes(
                        command
                            .frame
                            .get("schemes")
                            .context("set_host_uri_schemes requires schemes")?,
                    )
                    .map_err(anyhow::Error::msg)?;
                self.output
                    .response(command, Some(json!({"schemes":schemes})), None);
            }
            "negotiate_protocol" => {
                if command
                    .frame
                    .get("protocolVersion")
                    .and_then(WireValue::as_number)
                    != Some(2.0)
                {
                    bail!("Unsupported RPC protocol version; expected 2");
                }
                self.output
                    .response(command, Some(json!({"protocolVersion":2})), None);
            }
            "get_state" => {
                self.sessions.provider.refresh_usage_if_stale(
                    &self.config.model,
                    self.session.header["id"].as_str().map(str::to_owned),
                );
                let state = self.state().await;
                self.output.response(command, Some(state), None);
            }
            "get_messages" => self.output.response_wire(
                command,
                Some(WireValue::object(vec![(
                    "messages",
                    WireValue::Array(self.session.messages.lock().unwrap().clone()),
                )])),
                None,
                None,
            ),
            "get_messages_page" => {
                use ara_rpc::messages::{
                    RPC_MESSAGES_PAGE_BUSY_ERROR, RpcMessageSnapshot, RpcMessagesPageOptions,
                    page_rpc_messages,
                };
                if self.active.is_some() || self.auto.busy() || self.maintenance.is_some() {
                    self.output.response_wire(
                        command,
                        None,
                        Some(RPC_MESSAGES_PAGE_BUSY_ERROR.into()),
                        Some("session_busy"),
                    );
                } else {
                    let journal = self.session.journal.lock().await;
                    let messages = self.session.messages.lock().unwrap().clone();
                    let snapshot = RpcMessageSnapshot {
                        session_id: self.session.header["id"]
                            .as_str()
                            .context("Session identity is missing")?
                            .into(),
                        leaf_id: journal.leaf_id().map(Into::into),
                        message_count: messages.len(),
                    };
                    match page_rpc_messages(
                        &messages,
                        &snapshot,
                        RpcMessagesPageOptions {
                            cursor: command.frame.get("cursor"),
                            limit: command.frame.get("limit"),
                        },
                    ) {
                        Ok(page) => {
                            self.output
                                .response_wire(command, Some(page.into()), None, None)
                        }
                        Err(error) => self.output.response_wire(
                            command,
                            None,
                            Some(error.message),
                            error.code,
                        ),
                    }
                }
            }
            "get_session_stats" => {
                let journal = self.session.journal.lock().await;
                let messages = if journal
                    .branch()
                    .iter()
                    .any(|entry| entry.kind == "compaction")
                {
                    let projection = journal
                        .compacted_context_projection()
                        .context("projecting compacted Session statistics")?;
                    let entries = journal
                        .entries()
                        .iter()
                        .map(|entry| (entry.id.as_str(), entry))
                        .collect::<std::collections::HashMap<_, _>>();
                    projection
                        .items
                        .into_iter()
                        .map(|item| -> Result<WireValue> {
                            match item {
                                ara_session::CompactedContextItem::Summary(_) => {
                                    Ok(wire(json!({"role":"compactionSummary"})))
                                }
                                ara_session::CompactedContextItem::Message(sourced) => {
                                    if let Some(entry) = entries.get(sourced.entry_id.as_str())
                                        && let Some(public) = Session::public_entry_message(
                                            entry,
                                            &self.session.projector,
                                        )?
                                    {
                                        return Ok(public);
                                    }
                                    Ok(wire(
                                        AgentEvent::MessageEnd {
                                            message: sourced.message.into(),
                                        }
                                        .full()?["message"]
                                            .clone(),
                                    ))
                                }
                            }
                        })
                        .collect::<Result<Vec<_>>>()?
                } else {
                    self.session.messages.lock().unwrap().clone()
                };
                let usages = journal
                    .model_usage_records()
                    .map(|record| serde_json::to_value(record.usage).expect("usage serializes"))
                    .collect::<Vec<_>>();
                let mut stats = public_wire_session_stats(&messages, &usages);
                stats["sessionId"] = self.session.header["id"].clone();
                if let Some(file) = &self.session.file {
                    stats["sessionFile"] = json!(file);
                }
                self.output.response(command, Some(stats), None);
            }
            "get_branch_messages" => {
                let journal = self.session.journal.lock().await;
                let messages = journal
                    .entries()
                    .iter()
                    .map(branch_user_message)
                    .collect::<Result<Vec<_>>>()?
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>();
                self.output
                    .response(command, Some(json!({"messages":messages})), None);
            }
            "set_session_name" => {
                let name = command.string("name")?;
                let name = ara_prompt::js::trim(&name);
                if name.is_empty() {
                    bail!("Session name cannot be empty");
                }
                let mut journal = self.session.journal.lock().await;
                if !journal.set_session_name(name, "user")? {
                    bail!("Session name cannot be empty");
                }
                let applied = journal.title().title.clone();
                *self.session.name.lock().unwrap() = Some(applied);
                self.output.response(command, None, None);
            }
            "get_last_assistant_text" => {
                let text = last_assistant_wire_text(&self.session.messages.lock().unwrap());
                let data = text.map_or_else(
                    || WireValue::object(vec![]),
                    |text| WireValue::object(vec![("text", WireValue::String(text))]),
                );
                self.output.response_wire(command, Some(data), None, None);
            }
            "get_available_commands" => self.output.response(
                command,
                Some(json!({"commands":self.available_commands()})),
                None,
            ),
            "set_steering_mode" | "set_follow_up_mode" => {
                let mode = command.queue_mode()?;
                if command.kind == "set_steering_mode" {
                    self.agent.set_steering_mode(mode);
                } else {
                    self.agent.set_follow_up_mode(mode);
                }
                self.output.response(command, None, None);
            }
            "set_interrupt_mode" => {
                let mode = match command.string("mode")?.as_str() {
                    "immediate" => ara_agent::InterruptMode::Immediate,
                    "wait" => ara_agent::InterruptMode::Wait,
                    _ => bail!("mode must be immediate or wait"),
                };
                super::rpc_host_settings::store_interrupt_mode(
                    &super::ara_home().join("agent"),
                    mode,
                )?;
                self.config.interrupt_mode = mode;
                self.agent.set_interrupt_mode(mode);
                self.output.response(command, None, None);
            }
            "steer" | "follow_up" => {
                let message = self
                    .session
                    .projector
                    .new_legacy_message(serde_json::to_value(command.message()?)?, None)?;
                if command.kind == "steer" {
                    self.agent
                        .steer_raw_with_source(message, ara_agent::SteeringSource::User)?;
                } else {
                    self.agent.follow_up_raw(message)?;
                }
                self.drain_queues = true;
                self.output.response(command, None, None);
            }
            "abort" => {
                self.abort().await;
                self.output.response(command, None, None);
            }
            "new_session" => {
                let parent = command
                    .frame
                    .get("parentSession")
                    .map(|_| command.string("parentSession"))
                    .transpose()?;
                self.new_session(parent.as_deref()).await?;
                self.emit_available_commands();
                self.output
                    .response(command, Some(json!({"cancelled":false})), None);
            }
            "peer_control" => {
                let peers=self.sessions.peers.clone().context("Peer capability unavailable")?;let runtime=self.peer_runtime.clone().context("Current Peer owner missing")?;
                let operation=command.string("operation")?;
                let data=match operation.as_str(){
                    "list"=>serde_json::to_value(peers.coordinator.registry.list(&runtime.peer.root))?,
                    "cancel"=>{let target=command.string("target")?;let output=self.output.clone();let command=Command{id:command.id.clone(),kind:command.kind.clone(),frame:command.frame.clone()};tokio::spawn(async move{match peers.coordinator.cancel(&runtime.peer.root,&runtime.peer.id,&target).await{Ok(cancelled)=>output.response(&command,Some(json!({"cancelled":cancelled})),None),Err(error)=>output.response(&command,None,Some(error.to_string()))}});return Ok(());},
                    #[cfg(feature="test-fixture")]
                    "hub_refresh"=>{self.set_host_tools(self.host_tools.clone()).await?;json!({"refreshed":true})},
                    #[cfg(feature="test-fixture")]
                    "hub_self_probe"|"hub_direct_probe"=>{
                        use ara_agent::ActualExecuteRequest;
                        use ara_rpc::value_heap::{ValueError,ValueSnapshot};
                        anyhow::ensure!(self.active.is_none(),"Self Native probe requires an idle actual Host");
                        let native=self.sessions.tool_owner.native.as_ref().context("Actual Native self owner missing")?;
                        let binding=native.builtin.iter().find(|entry|entry.kind==ara_cli::actual_tools::BuiltinKind::Write).context("Actual Native WRITE binding missing")?.binding.clone();
                        let values=native.prepared.values().clone();let heap=values.heap();let receiver=binding.source.clone();
                        let self_send=operation=="hub_self_probe";let target=if self_send{runtime.peer.id.clone()}else{command.string("target")?};
                        let args=heap.import_serde(&json!({"path":format!("agent://{target}"),"content":"original immediate Native send body"}))?;
                        let call=heap.import_serde(&json!({"type":"toolCall","id":"hub-native-self","name":"write"}))?;
                        heap.define_own_data(&call,"arguments".into(),args.clone(),values.runtime())?;
                        let mut assistant=serde_json::to_value(ara_ai::AssistantMessage::empty("fixture","fixture","fixture"))?;
                        assistant["role"]="assistant".into();assistant["stopReason"]="toolUse".into();
                        let message=heap.import_serde(&assistant)?;heap.define_own_data(&message,"content".into(),heap.array(vec![call.clone()])?,values.runtime())?;
                        let owner=AgentMessage::from_graph(message,None)?;let tool_context=heap.object()?;let cancel=CancellationToken::new();
                        let driver=ara_cli::native_run::NativeRun::begin(native.prepared.clone(),cancel.clone())?;
                        native.prepared.require_tool_hooks()?.bind_run_context(&values,&tool_context)?;
                        let request=ActualExecuteRequest{callable:heap.get(&receiver,&"execute".into(),values.runtime())?,receiver,arguments_slot:owner.current_arguments_slot(&call)?,owner,tool_call:call.clone(),call_id:"hub-native-self".into(),args:args.clone(),values:values.clone(),cancel,on_update:Arc::new(|_|Ok(())),tool_context};
                        let outcome=driver.drive(binding.capability.actual_invoker().context("Actual Native WRITE invoker missing")?.invoke_settled(request)).await.and_then(|result|result);
                        fn diagnostic(error:&ara_agent::ActualProviderError,values:&SessionValues)->String{
                            match error{
                                ara_agent::ActualProviderError::Value(ValueError::Thrown(value))=>{
                                    values.heap().get(value,&"message".into(),values.runtime()).ok().and_then(|message|match values.heap().snapshot(&message).ok()?{ValueSnapshot::String(text)=>text.to_utf8().ok(),_=>None}).unwrap_or_else(||error.to_string())
                                },
                                ara_agent::ActualProviderError::Compound{primary,..}=>diagnostic(primary,values),
                                _=>error.to_string(),
                            }
                        }
                        let (error,result)=match outcome{
                            Ok(result)=>{let result=heap.export_serde(&result)?;if self_send{anyhow::ensure!(result["isError"]==true,"Actual Native self WRITE succeeded");(result["content"].to_string(),result)}else{anyhow::ensure!(result["isError"]!=true,"Actual Native direct WRITE failed");(String::new(),result)}},
                            Err(error)=>{anyhow::ensure!(self_send,"Actual Native direct WRITE threw: {error}");(diagnostic(&error,&values),Value::Null)},
                        };
                        if self_send{anyhow::ensure!(error.contains("yourself"),"Actual Native self refusal lost target diagnostic: {error}");}
                        anyhow::ensure!(heap.get(&call,&"arguments".into(),values.runtime())?.same_identity(&args),"Native call lost original arguments");
                        json!({"refused":self_send,"error":error,"nativeResult":result,"nativeGraphCall":true,"modelGenerated":false})
                    },
                    #[cfg(feature="test-fixture")]
                    "hub_missing_sender"=>{
                        let bad=ara_cli::host_peers::agent_uri::AgentUriPort::with_eligibility(peers.coordinator.clone(),runtime.peer.root.clone(),"missing-authoritative-sender",self.session.file.clone(),self.sessions.messaging_eligibility(0));
                        let port:Arc<dyn ara_tools::ContentUriPort>=bad;
                        self.sessions.tool_owner.context.as_ref().context("Hub actual ToolContext missing")?.set_uri_port(port);
                        json!({"senderMissing":true})
                    },
                    #[cfg(feature="test-fixture")]
                    "cold_non_peer"=>{
                        let path=PathBuf::from(command.string("target")?);
                        let peers=self.sessions.peers.take();let root=self.sessions.peer_root.take();let attachment=self.sessions.peer_attachment.take();
                        let opened:Result<super::child_host::ChildHost>=async{
                            let parent=self.session.journal.lock().await;
                            super::child_host::ChildHostFactory::new(&self.sessions,&parent)?.open(&path,&self.connection).await
                        }.await;
                        self.sessions.peers=peers;self.sessions.peer_root=root;self.sessions.peer_attachment=attachment;
                        let child=opened?;let state=child.handle().call(NativeAction::State).await;
                        let closed=child.close().await;let state=state?;closed?;state
                    },
                    #[cfg(feature="test-fixture")]
                    "cold_publication_fault"=>{
                        anyhow::ensure!(self.active.is_none(),"Cold publication control requires a settled original Run");
                        anyhow::ensure!(self.cold_publication_fault.lock().unwrap().is_none(),"Cold publication control already armed");
                        let donor=ara_cli::native_factory::PreparedNativeSession::prepare()?;
                        let context=self.sessions.tool_owner.context.as_ref().context("Cold publication control needs actual ToolContext")?.clone();
                        let builtin=ara_cli::actual_tools::BuiltinToolSource::new(donor.values(),context,&[])?;
                        let array=donor.values().heap().array(Vec::new())?;
                        let snapshot=ara_cli::native_factory::catalog::NativeCatalogSnapshot::new(
                            donor.values(),array,Vec::new(),ara_cli::native_factory::catalog::NativeCatalogSources{builtin,mcp:None,rpc:None})?;
                        let owner_before=self.cold_adoption_binding();
                        *self.cold_publication_fault.lock().unwrap()=Some(ColdPublicationFault{
                            _donor:donor,snapshot,owner_before,prepared_primary:None,
                            publication_attempted:false,native_error:None});
                        json!({"armed":true,"fault":"actual foreign-realm Native catalog"})
                    },
                    #[cfg(feature="test-fixture")]
                    "cold_publication_observation"=>{
                        // Always disarm before the next real model request.
                        let fault=self.cold_publication_fault.lock().unwrap().take().context("Cold publication control was not armed")?;
                        json!({"ownerBefore":fault.owner_before,"ownerAfter":self.cold_adoption_binding(),
                            "preparedPrimary":fault.prepared_primary,"publicationAttempted":fault.publication_attempted,
                            "nativeError":fault.native_error})
                    },
                    #[cfg(feature="test-fixture")]
                    "cold_new"=>{self.new_session(None).await?;json!({"adopted":true})},
                    #[cfg(feature="test-fixture")]
                    "cold_thinking"=>{self.set_thinking_level(&command.string("context")?).await?;json!({"changed":true})},
                    #[cfg(feature="test-fixture")]
                    "cold_side_usage"=>{
                        let factory=self.sessions.provider.borrowed_side_factory();
                        let id=self.session.header["id"].as_str().context("Cold actual Session id missing")?;
                        let side=factory.route.bind_side_request(factory.client.clone(),&format!("{id}:controlled-side"));
                        let context=ara_ai::Context{system_prompt:vec!["controlled scoped side task".into()],messages:vec![ara_ai::Message::User(ara_ai::UserMessage::text("scoped side actual request"))],tools:None};
                        let mut stream=side.stream(&self.config.model,&context,ara_ai::CallOptions{cancel:self.connection.clone(),..Default::default()});
                        let mut terminals=Vec::new();while let Some(event)=stream.recv().await{if event.is_terminal(){terminals.push(event.partial().clone());}}
                        anyhow::ensure!(terminals.len()==1&&terminals[0].stop_reason==ara_ai::StopReason::Stop,"Cold side actual stream did not finish successfully");
                        let account=factory.shared_account.as_ref().context("Cold global usage account missing")?;
                        let scope=factory.agent_context.as_ref().context("Cold scoped side context missing")?;
                        let options=ara_cli::auth_storage::FetchUsageReportsOptions{context:ara_cli::auth_storage::AuthRequestContext{session_scope:Some(scope.auth_scope.clone()),session_id:Some(id.into()),model_id:Some(self.config.model.id.clone()),..Default::default()},..Default::default()};
                        let reports=account.fetch_usage_reports_with_options(&options,&self.connection).await?.context("Cold global usage reports missing")?;
                        let global_accounts=account.list_oauth_accounts(&self.config.model.provider,Some(id))?;
                        json!({"sideText":terminals[0].text(),"actualSessionId":id,"scope":scope.receipt(),"reports":reports,"globalAccounts":global_accounts})
                    },
                    #[cfg(feature="test-fixture")]
                    "cold_key"=>{
                        let registry=self.sessions.provider.registry.as_ref().context("Cold scoped Registry missing")?;
                        let key=registry.get_api_key_for_provider(&self.config.model.provider.as_str().into(),None,&self.connection).await?;
                        let key=key.as_ref().and_then(|value|value.to_utf8().ok());
                        let identity=match key.as_deref(){Some("cold-synthetic-A")=>"A",Some("cold-synthetic-B")=>"B",Some("cold-key-K")|Some("sk-peer-loopback-controlled-fixture")=>"K",None=>"NONE",_=>"UNKNOWN"};
                        json!({"identity":identity,"actualSessionId":self.session.header["id"],"registry":registry.session_scope_receipt()})
                    },
                    #[cfg(feature="test-fixture")]
                    "cold_pin"=>{
                        let id=command.string("target")?.parse::<i64>()?;
                        let account=self.sessions.provider.shared_account.as_ref().context("Cold shared account owner missing")?;
                        let session=self.session.header["id"].as_str().context("Cold actual Session id missing")?;
                        anyhow::ensure!(account.pin_session_oauth_account(&self.config.model.provider,session,id,None)?,"Cold fixture account could not pin actual assignment");
                        json!({"pinnedCredentialId":id,"sessionId":session})
                    },
                    "task_cold_probe"=>{
                        let id=command.string("target")?;let peers=self.sessions.peers.as_ref().context("Cold probe requires actual Peer Host")?;
                        let root=self.peer_runtime.as_ref().context("Cold probe actual root missing")?.peer.root.clone();
                        let before=peers.coordinator.registry.get(&root,&id).context("Cold probe actual Task missing")?;
                        let file=before.session_file.as_ref().context("Cold probe actual transcript missing")?;
                        let stored=SessionJournal::open_with_blob_directory(file,&super::ara_blobs_directory())?;
                        let cwd=PathBuf::from(stored.header()["cwd"].as_str().context("Cold probe cwd missing")?);
                        let journal=self.session.journal.lock().await;let factory=super::child_host::ChildHostFactory::new(&self.sessions,&journal)?;
                        let error=match factory.open_peer(before.clone(),&self.connection).await{
                            Err(error)=>format!("{error:#}"),Ok(child)=>{let _=child.close().await;bail!("Cold isolated factory unexpectedly produced a runtime without ownership");}
                        };
                        let after=peers.coordinator.registry.get_exact(&before.key).context("Cold probe replaced original exact Task ref")?;
                        json!({"rejected":true,"error":error,"before":before,"after":after,"cwdExists":cwd.is_dir()})
                    },
                    "create"=>{let raw=command.frame.get("init").context("Peer create requires init")?;let init=serde_json::from_value(ara_session::native_checked_serde(raw)?)?;let context=command.frame.get("context").map(|_|command.string("context")).transpose()?.unwrap_or_default();let structured=match command.frame.get("structured"){Some(WireValue::Bool(value))=>*value,_=>true};serde_json::to_value(self.peer_create(init,context,structured).await?)?},
                    _=>bail!("peer_control operation must be create, list or cancel"),
                };self.output.response(command,Some(data),None);
            }
            "switch_session" => {
                let path = PathBuf::from(command.string("sessionPath")?);
                let cancelled = self.switch_session(path).await?;
                if !cancelled {
                    self.emit_available_commands();
                }
                self.output
                    .response(command, Some(json!({"cancelled":cancelled})), None);
            }
            "branch" => {
                let text = self.branch(&command.string("entryId")?).await?;
                self.emit_available_commands();
                self.output
                    .response(command, Some(json!({"text":text,"cancelled":false})), None);
            }
            "prompt" => {
                let text = command.string("message")?;
                let deadline = self
                    .max_time
                    .map(|seconds| Instant::now() + Duration::from_secs_f64(seconds.max(0.0)));
                let prepared = super::skill_command::prepare(
                    &text,
                    &text,
                    &self.skills,
                    &self.connection,
                    deadline,
                )
                .await
                .map_err(|error| match error {
                    super::skill_command::PreparationError::Load(error) => {
                        anyhow::anyhow!("Failed to load Skill: {error}")
                    }
                    super::skill_command::PreparationError::Cancelled => {
                        anyhow::anyhow!("Request was aborted")
                    }
                    super::skill_command::PreparationError::Deadline => {
                        anyhow::anyhow!("Deadline exceeded")
                    }
                })?;
                let recognized = prepared.is_some();
                // Resolve the registered Skill before ordinary image parsing.
                // Fixed RPC ignores images entirely for this entrypoint.
                let input = match prepared {
                    Some(prompt) => self
                        .session
                        .projector
                        .new_custom_input(prompt.event_message())?,
                    None => self
                        .session
                        .projector
                        .new_legacy_message(serde_json::to_value(command.message()?)?, None)?,
                };
                let queue = if let Some(value) = command.frame.get("streamingBehavior") {
                    match value.as_string().and_then(|s| s.to_utf8().ok()).as_deref() {
                        Some("steer") => Some(true),
                        Some("followUp") => Some(false),
                        _ => bail!("streamingBehavior must be steer or followUp"),
                    }
                } else {
                    recognized.then_some(true)
                };
                self.output.response(
                    command,
                    recognized.then(|| json!({"agentInvoked":true})),
                    None,
                );
                self.drain_queues = true;
                if self.maintenance.is_some() {
                    if queue == Some(false) {
                        self.agent.follow_up_raw(input)?;
                    } else {
                        self.agent
                            .steer_raw_with_source(input, ara_agent::SteeringSource::User)?;
                    }
                } else if (self.active.is_some() || self.auto.busy())
                    && let Some(steer) = queue
                {
                    if steer {
                        self.agent
                            .steer_raw_with_source(input, ara_agent::SteeringSource::User)?;
                    } else {
                        self.agent.follow_up_raw(input)?;
                    }
                } else {
                    if self.active.is_none() && !self.auto.busy() {
                        self.maintenance_continue = None;
                        self.finish_retry(None, Some("Retry cancelled".into()), false)
                            .await;
                    }
                    self.start(
                        Some(input),
                        Command {
                            id: command.id.clone(),
                            kind: command.kind.clone(),
                            frame: command.frame.clone(),
                        },
                    )
                    .await?;
                }
            }
            "abort_and_prompt" => {
                let message = self
                    .session
                    .projector
                    .new_legacy_message(serde_json::to_value(command.message()?)?, None)?;
                self.abort().await;
                self.output.response(command, None, None);
                self.drain_queues = true;
                self.start(
                    Some(message),
                    Command {
                        id: command.id.clone(),
                        kind: command.kind.clone(),
                        frame: command.frame.clone(),
                    },
                )
                .await?;
            }
            "extension_ui_response"
            | "host_tool_result"
            | "host_tool_update"
            | "host_uri_result" => {
                bail!("RPC side channel is not implemented yet");
            }
            "" => bail!("RPC command requires a string type"),
            _ => bail!("RPC command {} is not implemented yet", command.kind),
        }
        Ok(())
    }
}

fn interrupt_mode_name(mode: ara_agent::InterruptMode) -> &'static str {
    match mode {
        ara_agent::InterruptMode::Immediate => "immediate",
        ara_agent::InterruptMode::Wait => "wait",
    }
}

fn mode_name(mode: QueueMode) -> &'static str {
    match mode {
        QueueMode::All => "all",
        QueueMode::OneAtATime => "one-at-a-time",
    }
}

fn branch_user_message(entry: &ara_session::Entry) -> Result<Option<Value>> {
    if entry.kind != "message"
        || !entry
            .observed_message_field(&["role"])?
            .as_ref()
            .and_then(WireValue::as_string)
            .is_some_and(|role| role.equals_ascii("user"))
    {
        return Ok(None);
    }
    let text = entry_user_text(entry)?;
    Ok((!text.is_empty()).then(|| json!({"entryId":entry.id,"text":text})))
}

fn entry_user_text(entry: &ara_session::Entry) -> Result<String> {
    let Some(content) = entry.observed_message_field(&["content"])? else {
        return Ok(String::new());
    };
    if let Some(text) = content.as_string() {
        return text
            .to_utf8()
            .map_err(|_| anyhow::anyhow!("RPC branch text is not representable in UTF8"));
    }
    let mut text = String::new();
    for block in content.as_array().into_iter().flatten() {
        if block
            .get("type")
            .and_then(WireValue::as_string)
            .is_some_and(|kind| kind.equals_ascii("text"))
            && let Some(value) = block.get("text").and_then(WireValue::as_string)
        {
            text.push_str(
                &value
                    .to_utf8()
                    .map_err(|_| anyhow::anyhow!("RPC branch text is not representable in UTF8"))?,
            );
        }
    }
    Ok(text)
}

fn user_message_text(message: &Value) -> String {
    let content = &message["content"];
    content.as_str().map(str::to_owned).unwrap_or_else(|| {
        content
            .as_array()
            .into_iter()
            .flatten()
            .filter(|block| block["type"] == "text")
            .filter_map(|block| block["text"].as_str())
            .collect::<String>()
    })
}

fn public_wire_is(root: &WireValue, key: &str, expected: &str) -> bool {
    root.get(key)
        .and_then(WireValue::as_string)
        .is_some_and(|text| text.equals_ascii(expected))
}

/// Statistics observe only their existing scalar fields, never business text.
fn public_wire_session_stats(messages: &[WireValue], model_usage: &[Value]) -> Value {
    let numeric = |value: Option<&WireValue>| -> Value {
        value
            .and_then(WireValue::as_number)
            .map_or(Value::Null, |number| {
                serde_json::from_str(&WireValue::Number(number).stringify())
                    .expect("numeric wire JSON")
            })
    };
    let observed = messages.iter().map(|message| {
        let role = ["user", "assistant", "toolResult"].into_iter().find(|role| public_wire_is(message, "role", role));
        let content = message.get("content").and_then(WireValue::as_array).into_iter().flatten()
            .map(|block| if public_wire_is(block, "type", "toolCall") { json!({"type":"toolCall"}) } else { json!({}) })
            .collect::<Vec<_>>();
        let usage = message.get("usage");
        let mut fields = serde_json::Map::new();
        for name in ["input", "output", "reasoningTokens", "cacheRead", "cacheWrite", "totalTokens"] {
            fields.insert(name.into(), numeric(usage.and_then(|usage| usage.get(name))));
        }
        fields.insert("cost".into(), json!({"total":numeric(usage.and_then(|usage| usage.get("cost")).and_then(|cost| cost.get("total")))}));
        json!({"role":role,"content":content,"usage":Value::Object(fields)})
    }).collect::<Vec<_>>();
    session_stats_with_model_usage(&observed, model_usage)
}

#[cfg(test)]
fn session_stats(messages: &[Value]) -> Value {
    session_stats_with_model_usage(messages, &[])
}

fn session_stats_with_model_usage(messages: &[Value], model_usage: &[Value]) -> Value {
    let count = |role: &str| {
        messages
            .iter()
            .filter(|message| message["role"] == role)
            .count()
    };
    let assistants = messages
        .iter()
        .filter(|message| message["role"] == "assistant")
        .collect::<Vec<_>>();
    let usage = assistants
        .iter()
        .map(|message| &message["usage"])
        .chain(model_usage.iter())
        .collect::<Vec<_>>();
    // Any missing provider bucket makes that aggregate unknown. A successful
    // response without pricing metadata is not a zero-cost response.
    let sum = |field: &str| {
        usage
            .iter()
            .try_fold(0u64, |sum, usage| sum.checked_add(usage[field].as_u64()?))
    };
    let cost = usage.iter().try_fold(0.0, |sum, usage| {
        Some(sum + usage["cost"]["total"].as_f64()?)
    });
    let tool_calls = assistants
        .iter()
        .map(|message| {
            message["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|block| block["type"] == "toolCall")
                .count()
        })
        .sum::<usize>();
    json!({"userMessages":count("user"),"assistantMessages":assistants.len(),
        "toolCalls":tool_calls,"toolResults":count("toolResult"),"totalMessages":messages.len(),
        "tokens":{"input":sum("input"),"output":sum("output"),"reasoning":sum("reasoningTokens"),
            "cacheRead":sum("cacheRead"),"cacheWrite":sum("cacheWrite"),"total":sum("totalTokens")},
        "cost":cost,"premiumRequests":if usage.is_empty() {json!(0)} else {Value::Null}})
}

#[cfg(test)]
fn last_assistant_text(messages: &[Value]) -> Option<String> {
    let wire_messages = messages.iter().cloned().map(wire).collect::<Vec<_>>();
    last_assistant_wire_text(&wire_messages)
        .map(|text| text.to_utf8().expect("legacy input is scalar text"))
}

fn last_assistant_wire_text(messages: &[WireValue]) -> Option<ara_rpc::WireString> {
    let assistant = messages.iter().rev().find(|message| {
        public_wire_is(message, "role", "assistant")
            && !(public_wire_is(message, "stopReason", "aborted")
                && message
                    .get("content")
                    .and_then(WireValue::as_array)
                    .is_some_and(<[WireValue]>::is_empty))
    })?;
    let mut units = Vec::new();
    for block in assistant
        .get("content")
        .and_then(WireValue::as_array)
        .into_iter()
        .flatten()
    {
        if public_wire_is(block, "type", "text")
            && let Some(text) = block.get("text").and_then(WireValue::as_string)
        {
            units.extend_from_slice(text.units());
        }
    }
    let space = |unit: &u16| char::from_u32(u32::from(*unit)).is_some_and(ara_prompt::js::is_space);
    let start = units.iter().position(|unit| !space(unit))?;
    let end = units.iter().rposition(|unit| !space(unit)).unwrap() + 1;
    Some(ara_rpc::WireString::from_units(units[start..end].to_vec()))
}

// Keep the RPC actor behind one heap-owned future. Pure forwarding async
// adapters otherwise retain large debug poll temporaries for every actor poll.
pub fn run(
    config: AgentConfig,
    skills: Vec<LoadedSkill>,
    messages: Vec<AgentMessage>,
    journal: Option<SessionJournal>,
    header: Value,
    max_time: Option<f64>,
    sessions: SessionFactory,
) -> Pin<Box<impl Future<Output = Result<i32>> + Send>> {
    serve(
        tokio::io::stdin(),
        tokio::io::stdout(),
        config,
        skills,
        messages,
        journal,
        header,
        max_time,
        sessions,
    )
}

#[allow(clippy::too_many_arguments)]
fn serve<R, W>(
    input: R,
    writer: W,
    config: AgentConfig,
    skills: Vec<LoadedSkill>,
    messages: Vec<AgentMessage>,
    journal: Option<SessionJournal>,
    header: Value,
    max_time: Option<f64>,
    sessions: SessionFactory,
) -> Pin<Box<impl Future<Output = Result<i32>> + Send>>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    Box::pin(serve_owned(
        input, writer, config, skills, messages, journal, header, max_time, sessions, None,
    ))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_native_session(
    config: AgentConfig,
    skills: Vec<LoadedSkill>,
    messages: Vec<AgentMessage>,
    journal: Option<SessionJournal>,
    header: Value,
    max_time: Option<f64>,
    sessions: SessionFactory,
    frames: NativeFrameSink,
) -> NativeSessionRuntime {
    let (sender, input) = mpsc::unbounded_channel();
    let connection = CancellationToken::new();
    let (refresh_epoch, _) = tokio::sync::watch::channel(0u64);
    let failure = Arc::new(Mutex::new(None));
    let cancel_receipt = Arc::new(NativeCancelReceipt::default());
    let handle = NativeSessionHandle {
        sender,
        connection: connection.clone(),
        refresh_epoch,
        refresh_blocked: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        failure: failure.clone(),
        cancel_receipt: cancel_receipt.clone(),
    };
    let io = NativeIo {
        input,
        pending: Arc::new(Mutex::new(Default::default())),
        frames,
        connection,
        failure,
        cancel_receipt,
    };
    let task = tokio::spawn(Box::pin(serve_owned(
        tokio::io::empty(),
        tokio::io::sink(),
        config,
        skills,
        messages,
        journal,
        header,
        max_time,
        sessions,
        Some(io),
    )));
    NativeSessionRuntime { handle, task }
}

#[allow(clippy::too_many_arguments)]
async fn serve_owned<R, W>(
    input: R,
    writer: W,
    config: AgentConfig,
    skills: Vec<LoadedSkill>,
    messages: Vec<AgentMessage>,
    journal: Option<SessionJournal>,
    header: Value,
    max_time: Option<f64>,
    mut sessions: SessionFactory,
    native: Option<NativeIo>,
) -> Result<i32>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let compaction_policy = AutoCompactionPolicy::load(&super::ara_home().join("agent"))?;
    let mut retry_policy = RetryPolicy::load(&super::ara_home().join("agent"))?;
    if let Some(init)=journal.as_ref().map(SessionJournal::latest_child_init).transpose()?.flatten() {
        retry_policy=retry_policy.with_child_fallback(header["id"].as_str().context("Child header id missing")?,init.retry_fallback.as_ref());
    }
    let loop_guard_settings = LoopGuardSettings::load(&super::ara_home().join("agent"))?;
    let unexpected_stop_mode = UnexpectedStopMode::load(&super::ara_home().join("agent"))?;
    let tool_loop_settings_dir = super::ara_home().join("agent");
    let tool_loop_settings = ToolLoopGuardSettings::load(&tool_loop_settings_dir)?;
    let mut tool_loop_state = ToolLoopState::default();
    tool_loop_state.configure(tool_loop_settings);
    let connection = native
        .as_ref()
        .map(|io| io.connection.clone())
        .unwrap_or_default();
    let handoff_control = Arc::new(Mutex::new(handoff::HandoffControl::default()));
    let quota_control = Arc::new(Mutex::new(quota::QuotaControl::default()));
    let model_selection_control = Arc::new(Mutex::new(quota::QuotaControl::default()));
    let quota_admission = quota_control.clone();
    sessions.provider.set_quota_admission(Arc::new(move || {
        !quota_admission.lock().unwrap().stop_requested()
    }));
    let (output_tx, output_rx) = mpsc::unbounded_channel();
    let output = Output(output_tx);
    if let Some(controller) = &sessions.provider.reset_controller {
        let notices = output.clone();
        controller.set_notice_sink(Arc::new(move |level, message| {
            notices.frame(
                json!({"type":"notice","level":level,"source":"codex-auto-reset",
                "message":super::sanitize_text(message)}),
            );
        }));
    }
    let peer_enabled=sessions.peers.is_some()||matches!(sessions.args.mode,super::Mode::Rpc)||config.tools.iter().any(|tool|tool.definition().name=="task");
    let output_task = if let Some(io) = &native {
        let output = Arc::new(NativeOutput {
            stop_after_child_shutdown: peer_enabled || sessions.tool_owner.native.as_ref()
                .is_some_and(|owner| owner.child_contract.is_some()),
            pending: io.pending.clone(),
            frames: io.frames.clone(),
            connection: connection.clone(),
            failure: io.failure.clone(),
            cancel_receipt: io.cancel_receipt.clone(),
        });
        tokio::spawn(write_native_output(output_rx, output))
    } else {
        tokio::spawn(write_output(writer, output_rx, connection.clone()))
    };
    let emitter_output = output.clone();
    let emitter: Arc<dyn Fn(WireValue) + Send + Sync> =
        Arc::new(move |frame| emitter_output.send(OutputItem::Frame(frame)));
    let tool_bridge = ToolBridge::new(emitter.clone());
    let uri_bridge = UriBridge::new(emitter);
    let values = config
        .hooks
        .provider_context_binding()
        .and_then(|binding| binding.actual_values);
    let native_session = sessions.initial_native_session.take();
    if sessions.actual_values_factory.is_some() && native_session.is_none() {
        bail!("Configured Native Session factory has no prepared initial Session owner");
    }
    validate_native_session_values(&values, &native_session)?;
    if let Some(controller) = &sessions.context_controller {
        controller.stage_actual_values(values.clone())?;
    }
    validate_session_values(&config, sessions.context_controller.as_ref(), &values)?;
    validate_native_session_hooks(&config, &values, &native_session)?;
    let session = Session::new_with_agent_context(journal, header, &messages, values, native_session, sessions.agent_context.clone())?;
    session.bind_edit_session(&sessions.tool_owner).await?;
    *session.tool_owner.lock().unwrap() = sessions.tool_owner.clone();
    let messages = ara_cli::remote_compaction::route_raw_context(
        &*session.journal.lock().await,
        &session.projector,
        &config.model,
        sessions.provider.metadata.as_ref(),
        &compaction_policy.recovery_settings()?.remote,
        sessions.provider.route.remote_supports_images(),
    )?;
    if peer_enabled&&sessions.peers.is_none(){sessions.peers=Some(super::peer_host::BinaryPeerHost::new());}
    let (task_tx,mut task_rx)=mpsc::unbounded_channel();
    let task_dispatch=super::task_host::TaskDispatch{sender:task_tx,scheduler:Arc::new(super::task_host::TaskScheduler::default())};
    if let Some(native)=&sessions.tool_owner.native{
        super::task_host::bind(&task_dispatch,native,session.header["id"].as_str().context("Task Session id missing")?,&sessions.cwd,sessions.tool_owner.edit_session.clone())?;
    }
    let (peer_tx,mut peer_rx)=mpsc::unbounded_channel();
    let mut peer_runtime=None;
    if let Some(peers)=&sessions.peers {
        let journal=session.journal.lock().await;
        let parent=journal.latest_child_init()?.and_then(|init|init.extra.get("araChildParentId").and_then(Value::as_str).map(str::to_owned));
        let id=journal.session_id().to_owned();let root=sessions.peer_root.clone().unwrap_or_else(||id.clone());
        sessions.peer_root=Some(root.clone());
        #[cfg(feature="test-fixture")]
        super::peer_host::fixture_frontier("peer-before-bind",&format!("{root}:{id}")).await?;
        let runtime=peers.bind_actor(&root,&id,parent,session.file.clone(),peer_tx.clone(),sessions.peer_attachment.take().unwrap_or(ara_cli::host_peers::AttachmentExpectation::Absent))?;
        if let Some(native)=&sessions.tool_owner.native{peers.bind_native(native,&runtime,journal.is_persistent())?;}
        peer_runtime=Some(runtime);
    }
    // A served artifact router already contains vault/native fallbacks. Only
    // an explicit base is inherited; never save that served router as the next
    // logical Session's stable override skeleton.
    let supplied_base=sessions.tool_owner.uri_base.clone().or_else(||if sessions.tool_owner.uri_artifacts.is_none(){sessions.uri_port.take()}else{None});
    let base: Arc<dyn ara_tools::ContentUriPort> = match supplied_base {
        Some(port) => ara_cli::host_vault::compose_ports(uri_bridge.clone(), Some(port)),
        None => uri_bridge.clone(),
    };
    let peer_depth=super::rpc_host_settings::peer_task_depth(Some(&*session.journal.lock().await))?;
    sessions.tool_owner.uri_peer=if let (Some(peers),Some(runtime))=(&sessions.peers,&peer_runtime){Some(peers.uri(&runtime.peer.root,&runtime.peer.id,session.file.clone(),sessions.messaging_eligibility(peer_depth)))}else{None};
    let fallback = super::prepared_host_uri(sessions.tool_owner.vault.as_ref(), Some(&base),sessions.tool_owner.uri_peer.as_ref(), None)
        .expect("explicit Native URI base");
    let artifact_router = ara_cli::session_artifacts::ArtifactUriRouter::new(
        session.artifacts.clone(),
        Some(fallback),
    );
    if let Some(context) = &sessions.tool_owner.context {
        context.set_uri_port(artifact_router.clone());
    }
    sessions.uri_port = Some(artifact_router.clone());
    sessions.tool_owner.uri_base = Some(base);
    sessions.tool_owner.uri_artifacts = Some(artifact_router.clone());
    if let Some(plan) = &sessions.acp_plan {
        let live: Arc<dyn ara_tools::ContentUriPort> = artifact_router.clone();
        plan.bind_live_uri_port(Arc::downgrade(&live));
    }
    *session.tool_owner.lock().unwrap() = sessions.tool_owner.clone();
    let snapshot = Arc::new(RwLock::new(ExecutionSnapshot::from_config(&config)));
    let snapshot_publication =
        ara_cli::append_only_context_mode::SnapshotPublication::new(&snapshot);
    if let Some(controller) = &sessions.context_controller {
        snapshot_publication.publish(
            controller,
            ExecutionSnapshot::from_config(&config),
            sessions.provider.metadata.as_ref(),
        );
    }
    let (admission_tx, mut admission_rx) = mpsc::unbounded_channel();
    let (auto_done_tx, mut auto_done_rx) = mpsc::unbounded_channel();
    let route_generation = Arc::new(std::sync::atomic::AtomicU64::new(
        sessions.provider.route.generation(),
    ));
    let bash_target = Arc::new(Mutex::new(BashTarget {
        session: session.clone(),
        destination: BashDestination::Current,
    }));
    let (bash_done_tx, mut bash_done_rx) = mpsc::unbounded_channel();
    let bash_dispatcher = Arc::new(BashDispatcher {
        current: Mutex::new(bash_target.clone()),
        jobs: Mutex::new(std::collections::HashMap::new()),
        next_id: std::sync::atomic::AtomicU64::new(1),
        done: bash_done_tx,
        output: output.clone(),
        connection: connection.clone(),
        cwd: sessions.cwd.clone(),
    });
    let mut host = Host {
        #[cfg(feature="test-fixture")]
        cold_publication_fault: Mutex::new(None),
        task_dispatch:None,
        peer_runtime:None,peer_sender:None,peer_wake:None,peer_deferred:Default::default(),
        child_turn:None,
        agent: Agent::new_raw(config.clone(), messages)?,
        session,
        config,
        skills,
        max_time,
        output: output.clone(),
        connection: connection.clone(),
        active: None,
        handoff_control: handoff_control.clone(),
        quota_control: quota_control.clone(),
        model_selection_control: model_selection_control.clone(),
        maintenance: None,
        maintenance_continue: None,
        terminal_recovery: maintenance::TerminalRecoveryState::default(),
        local_context_rebase: None,
        #[cfg(test)]
        fixed_snapcompact_observation: None,
        sessions,
        tool_bridge: tool_bridge.clone(),
        uri_bridge: uri_bridge.clone(),
        artifact_router,
        host_tools: Vec::new(),
        snapshot,
        snapshot_publication,
        admission_tx,
        route_generation,
        drain_queues: true,
        bash_targets: vec![Arc::downgrade(&bash_target)],
        bash_target,
        bash_dispatcher: bash_dispatcher.clone(),
        pending_bash: Vec::new(),
        bash_error: None,
        compaction_policy,
        is_compacting: false,
        auto_compaction_pending: false,
        auto_compaction_checked: None,
        retry_policy,
        retry: None,
        fallback: fallback::FallbackState::default(),
        auto: auto::AutoState::with_done(auto_done_tx),
        model_usage_order: Arc::new(
            ara_cli::model_usage_order::SqliteModelUsageOrder::for_agent_dir(
                super::ara_home().join("agent"),
            ),
        ),
        model_cycle_scope: Vec::new(),
        prompt_generation: 0,
        loop_guard_settings,
        unexpected_stop_mode,
        header_continue: None,
        input_closed: false,
        tool_loop: Arc::new(Mutex::new(tool_loop_state)),
        tool_loop_settings_dir: Some(tool_loop_settings_dir),
    };
    host.task_dispatch=Some(task_dispatch);host.peer_runtime=peer_runtime;host.peer_sender=peer_enabled.then_some(peer_tx);
    host.initialize_model_cycle_scope()?;
    output.frame(
        json!({"type":"ready","protocolVersion":1,"supportedProtocolVersions":[1,2],
        "maxFrameBytes":MAX_RPC_FRAME_BYTES,"maxReassembledFrameBytes":MAX_RPC_REASSEMBLED_BYTES}),
    );
    host.emit_available_commands();
    host.initialize_thinking_selection().await?;
    host.sessions.provider.refresh_usage_if_stale(
        &host.config.model,
        host.session.header["id"].as_str().map(str::to_owned),
    );
    let (input_tx, mut input_rx) = mpsc::unbounded_channel();
    let reader_output = output.clone();
    let reader_cancel = connection.clone();
    let reader_handoff_control = handoff_control.clone();
    let reader_quota_control = quota_control.clone();
    let reader_model_selection_control = model_selection_control;
    let reader_start_control = host.auto.control.clone();
    let reader_task = tokio::spawn(async move {
        if let Some(mut io) = native {
            loop {
                let request = tokio::select! {
                    biased;
                    _ = reader_cancel.cancelled() => break,
                    request = io.input.recv() => request,
                };
                let Some(request) = request else {
                    break;
                };
                let owned = io.command(request);
                if let OwnedInput::Command(command) = &owned {
                    let mut gate = reader_start_control.lock().unwrap();
                    if auto::StartControl::pauses_start(command) {
                        gate.queue();
                    }
                    if handoff::HandoffControl::stops_handoff(command) {
                        reader_handoff_control.lock().unwrap().queue_stop();
                    }
                    if quota::QuotaControl::stops_quota(command) {
                        reader_quota_control.lock().unwrap().queue_stop();
                    }
                    if models::stops_selection(command) {
                        reader_model_selection_control.lock().unwrap().queue_stop();
                    }
                }
                if input_tx.send(owned).is_err() {
                    break;
                }
            }
            reader_handoff_control.lock().unwrap().close_input();
            reader_quota_control.lock().unwrap().close_input();
            reader_model_selection_control.lock().unwrap().close_input();
            tool_bridge.close("Native Host closed");
            uri_bridge.close_connection("Native Host closed");
            return Ok(());
        }
        let mut reader = RpcInputReader::new(input);
        loop {
            let next = tokio::select! {
                biased;
                _ = reader_cancel.cancelled() => break,
                item = reader.read_next() => item,
            };
            match next {
                Ok(Some(InputItem::Frame(frame))) => {
                    if tool_bridge.consume(&frame) || uri_bridge.consume(&frame) {
                        continue;
                    }
                    let command = Command::new(frame);
                    if command.kind == "bash" {
                        bash_dispatcher.dispatch(command);
                        continue;
                    }
                    let stops_handoff = handoff::HandoffControl::stops_handoff(&command);
                    let stops_quota = quota::QuotaControl::stops_quota(&command);
                    let stops_selection = models::stops_selection(&command);
                    let pauses_start = auto::StartControl::pauses_start(&command);
                    // Keep acceptance/enqueue under the same gate as deferred
                    // thinking publication and final main dispatch.
                    let sent = {
                        let mut gate = reader_start_control.lock().unwrap();
                        if pauses_start {
                            gate.queue();
                        }
                        if stops_handoff {
                            reader_handoff_control.lock().unwrap().queue_stop();
                        }
                        if stops_quota {
                            reader_quota_control.lock().unwrap().queue_stop();
                        }
                        if stops_selection {
                            reader_model_selection_control.lock().unwrap().queue_stop();
                        }
                        let sent = input_tx.send(OwnedInput::Command(command)).is_ok();
                        if !sent {
                            if pauses_start {
                                gate.consume();
                            }
                            if stops_handoff {
                                reader_handoff_control.lock().unwrap().consume_stop();
                            }
                            if stops_quota {
                                reader_quota_control.lock().unwrap().consume_stop();
                            }
                            if stops_selection {
                                reader_model_selection_control
                                    .lock()
                                    .unwrap()
                                    .consume_stop();
                            }
                        }
                        sent
                    };
                    if !sent {
                        break;
                    }
                }
                Ok(Some(InputItem::ParseError(error))) => reader_output.frame(
                    json!({"type":"response","command":"parse","success":false,"error":error}),
                ),
                Ok(None) => break,
                Err(error) => {
                    reader_handoff_control.lock().unwrap().close_input();
                    reader_quota_control.lock().unwrap().close_input();
                    reader_model_selection_control.lock().unwrap().close_input();
                    tool_bridge.close("RPC input disconnected");
                    uri_bridge.close_connection("RPC input disconnected");
                    reader_cancel.cancel();
                    return Err(error);
                }
            }
        }
        reader_handoff_control.lock().unwrap().close_input();
        reader_quota_control.lock().unwrap().close_input();
        reader_model_selection_control.lock().unwrap().close_input();
        // Unblock accepted work before the serial command owner drains/joins.
        tool_bridge.close("RPC host disconnected");
        uri_bridge.close_connection("RPC host disconnected");
        Ok(())
    });
    let mut eof = false;
    let mut native_idle = Vec::new();
    loop {
        // Register before reading pending jobs: notify_waiters carries no stored
        // permit and a completed child must wake an otherwise idle parent.
        let peer_job_notify=host.sessions.peers.as_ref().map(|peers|peers.coordinator.jobs.wake()).unwrap_or_else(||Arc::new(tokio::sync::Notify::new()));
        let peer_job_wait=peer_job_notify.notified();tokio::pin!(peer_job_wait);peer_job_wait.as_mut().enable();
        // Child publication commits after its actor RPC response. Register
        // before inspecting pending_publication so a pool clear cannot strand
        // an already deferred Peer wake in an otherwise idle actor.
        let child_contract_owner=host.sessions.tool_owner.native.as_ref().and_then(|native|native.child_contract.clone());
        let no_child_contract_wake=tokio::sync::Notify::new();
        let child_contract_wake=child_contract_owner.as_ref().map(|owner|owner.wake()).unwrap_or(&no_child_contract_wake);
        let child_contract_wait=child_contract_wake.notified();tokio::pin!(child_contract_wait);child_contract_wait.as_mut().enable();
        host.flush_pending_bash().await;
        host.resume_auto_start().await;
        host.drain_pending_auto_compaction(eof).await;
        host.reconcile_queues().await;
        // Waiting for completed ownership is independent of whether a Run
        // permits automatic queue drain or late diagnostics. Abort/failure
        // intentionally disable those policies but still settle this receipt.
        let native_settled = host.active.is_none()
            && !host.auto.busy()
            && host.retry.is_none()
            && host.header_continue.is_none()
            && host.maintenance.is_none()
            && host.maintenance_continue.is_none()
            && host.bash_dispatcher.is_empty();
        if native_settled {host.finish_peer_wake().await;host.flush_peer_parent_results().await;if !eof{host.start_deferred_peer().await;}}
        let native_settled=native_settled&&host.active.is_none()&&host.peer_wake.is_none();
        if let (Some(peers),Some(runtime))=(&host.sessions.peers,&host.peer_runtime){peers.dispatch_parent_completions(runtime);}
        if native_settled && !native_idle.is_empty() {
            let failure = host
                .bash_error
                .clone()
                .or_else(|| host.session.persistence_error.lock().unwrap().clone());
            if let Some(error) = failure {
                for command in native_idle.drain(..) {
                    host.output.response(&command, None, Some(error.clone()));
                }
            } else {
                let state = host.state().await;
                for command in native_idle.drain(..) {
                    host.output.response(&command, Some(state.clone()), None);
                }
            }
        }
        if eof && native_settled {
            break;
        }
        let edit_wake = host.session.edit_session().map(|owner| owner.wake());
        tokio::select! {
            biased;
            _ = &mut peer_job_wait => {},
            _ = &mut child_contract_wait => {},
            _ = async {match host.peer_wake.as_ref(){Some(wake)=>wake.cancel.cancelled().await,None=>pending().await}} => {host.abort().await;},
            _ = connection.cancelled() => {
                if let Some(reset_host) = &host.sessions.provider.reset_host { reset_host.close(); }
                host.abort().await;
                host.bash_dispatcher.abort();
                while !host.bash_dispatcher.is_empty() {
                    if let Some(completion) = bash_done_rx.recv().await {
                        host.completed_bash(completion).await;
                    }
                }
                host.flush_pending_bash().await;
                break;
            }
            _ = async { if let Some(wake)=&edit_wake { wake.notified().await } else { pending::<()>().await } },
                if host.late_idle_allowed() && !eof => {
                host.reconcile_late_diagnostics().await;
            }
            request = admission_rx.recv() => {
                if let Some(request) = request {
                    host.handle_usage_admission(request).await;
                }
            }
            completion = auto_done_rx.recv() => {
                if let Some(completion) = completion { host.complete_auto(completion); }
            }
            completion = bash_done_rx.recv() => {
                if let Some(completion) = completion {
                    host.completed_bash(completion).await;
                }
            }
            result = async {
                match &mut host.active {
                    Some(active) => (&mut active.task).await,
                    None => pending().await,
                }
            }, if host.active.is_some() => {
                let active = host.active.take().expect("selected active Run");
                Box::pin(host.completed(active, result)).await;
                host.sessions.provider.refresh_usage_if_stale(&host.config.model,
                    host.session.header["id"].as_str().map(str::to_owned));
            }
            result = async {
                match &mut host.maintenance {
                    Some(active) => (&mut active.task).await,
                    None => pending().await,
                }
            }, if host.maintenance.is_some() => {
                let active = host.maintenance.take().expect("selected context maintenance");
                Box::pin(host.completed_maintenance(active, result)).await;
            }
            _ = async {
                match &host.maintenance_continue {
                    Some(pending) => tokio::time::sleep_until(pending.deadline.into()).await,
                    None => pending().await,
                }
            }, if host.maintenance_continue.is_some() => {
                host.resume_maintenance_continue().await;
            }
            _ = async {
                match host.retry.as_ref().and_then(|retry| retry.deadline) {
                    Some(deadline) => tokio::time::sleep_until(deadline.into()).await,
                    None => pending().await,
                }
            }, if host.retry.as_ref().is_some_and(|retry| retry.deadline.is_some()) => {
                host.resume_retry().await;
            }
            _ = async {
                match host.header_continue.as_ref() {
                    Some(pending) => tokio::time::sleep_until(pending.deadline.into()).await,
                    None => pending().await,
                }
            }, if host.header_continue.is_some() => {
                host.resume_header_continue().await;
            }
            request = task_rx.recv() => {
                if let Some(request)=request{host.task_request(request).await;}
            }
            request = peer_rx.recv(), if host.sessions.peers.is_some() => {
                if let Some(request)=request{host.peer_request(request).await;}
            }
            command = input_rx.recv(), if !eof => {
                match command {
                    Some(OwnedInput::Command(command)) => host.handle(command).await,
                    Some(OwnedInput::NativeIdle(command)) => native_idle.push(command),
                    Some(OwnedInput::NativeChild(command,action))=>{
                        match host.child_action(action,&command).await {
                            Ok(value)=>host.output.response(&command,Some(value),None),
                            Err(error)=>host.output.response(&command,None,Some(error.to_string())),
                        }
                    },
                    Some(OwnedInput::NativeRefresh(command, decision)) => {
                        match host.refresh_acp_plan(&decision).await {
                            Ok(()) => host.output.response(&command, None, None),
                            Err(error) => {
                                decision.reject_before_publication(error.to_string());
                                host.output.response(&command, None, Some(error.to_string()));
                            },
                        }
                    },
                    None => {
                        eof = true;
                        host.input_closed = true;
                        if let Some(reset_host) = &host.sessions.provider.reset_host { reset_host.close(); }
                        host.abort_maintenance().await;
                        if host.header_continue.take().is_some() {
                            host.finish_retry(None, Some("Gemini continuation cancelled by EOF".into()), false).await;
                            host.drain_queues = false;
                        }
                        if let Some(active) = &host.active {
                            active.sink.header_guard.lock().unwrap().revoke();
                        }
                    },
                }
            }
        }
    }
    let input_result = reader_task.await.context("RPC input task");
    host.auto.settle_jobs().await;
    host.sessions.provider.settle_quota_feedback().await;
    if let Some(host) = &host.sessions.provider.reset_host {
        host.close();
    }
    if let Some(controller) = &host.sessions.provider.reset_controller {
        controller.close().await;
    }
    if let Some(lsp) = &host.sessions.tool_owner.lsp_startup {
        if let Err(error) = lsp.shutdown().await {
            eprintln!("ara: LSP shutdown failed: {error}");
        }
    }
    let mut failed = host
        .bash_error
        .clone()
        .or_else(|| host.session.persistence_error.lock().unwrap().clone());
    if let Some(native)=&host.sessions.tool_owner.native{native.peer_ingress.release_unappended();}
    if let Some(pending)=host.peer_wake.take(){let _=pending.reply.send(Err(anyhow::anyhow!("Peer actor closed before retained turn settlement")));}
    for pending in host.peer_deferred.drain(..){let _=pending.reply.send(Err(anyhow::anyhow!("Peer actor closed with unadmitted deferred messages")));}
    if let (Some(peers),Some(runtime))=(&host.sessions.peers,&host.peer_runtime){
        let root=peers.coordinator.registry.get_exact(&runtime.peer).is_some_and(|record|record.kind==ara_cli::host_peers::PeerKind::Main&&record.parent_id.is_none());
        if root {
            // Seal before closing owners and reject already queued callbacks;
            // descendant close operations run independently outside table locks.
            peers.coordinator.registry.seal_root(&runtime.peer.root,runtime);
            peers.coordinator.jobs.seal_root(&runtime.peer.root);
            while let Ok(request)=peer_rx.try_recv(){let _=request.reply.send(Err(anyhow::anyhow!("Root closed before Peer actor request admission")));}
            let mut own_unknown=false;let mut effect_receipts=Vec::new();
            if let Some(native)=&host.sessions.tool_owner.native {
                native.task_session.close();native.wait_session.close();native.bash_session.close();if let Some(service)=&native.service_session{service.close();}
                for receipt in native.bash_session.effects().settle().await {own_unknown|=receipt.uncertain();effect_receipts.push(format!("{receipt:?}"));}
            }
            match peers.release_root(runtime).await {
                Ok(mut receipt)=>{receipt.settled&=!own_unknown;host.output.frame(json!({"type":"peer_root_shutdown","receipt":receipt,"rootOwnedEffects":effect_receipts}));if !receipt.settled{host.session.persistence_error.lock().unwrap().get_or_insert_with(||"Root shutdown retains unresolved or unknown effects; no replay".into());}},
                Err(error)=>{host.session.persistence_error.lock().unwrap().get_or_insert_with(||format!("Root descendant shutdown failed: {error}"));},
            }
        }
        peers.unbind(runtime);
    }
    if let Some(native)=&host.sessions.tool_owner.native{native.task_session.close();}
    while let Ok(request)=task_rx.try_recv(){let _=request.reply.send(Err(anyhow::anyhow!("Task Host closed before request admission")));}
    failed=failed.or_else(||host.session.persistence_error.lock().unwrap().clone());
    output.frame(json!({"type":"session_shutdown"}));
    drop(host);
    drop(output);
    output_task.await.context("RPC output task")??;
    input_result?.context("reading RPC input")?;
    if let Some(error) = failed {
        eprintln!("ara: session persistence failed ({error}); restart from the journal");
        Ok(1)
    } else {
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use tokio::sync::oneshot;

    struct TerminalGate {
        inner: Arc<RunSink>,
        reached: Mutex<Option<oneshot::Sender<()>>>,
        release: tokio::sync::Mutex<Option<oneshot::Receiver<()>>>,
    }

    #[async_trait]
    impl AgentEventSink for TerminalGate {
        async fn emit_input(&self, input: AgentInput) {
            self.inner.emit_input(input).await;
        }

        async fn emit(&self, event: AgentEvent) {
            let terminal = matches!(
                event,
                AgentEvent::AgentEnd { .. } | AgentEvent::ActualAgentEnd { .. }
            );
            self.inner.emit(event).await;
            if terminal {
                self.reached
                    .lock()
                    .unwrap()
                    .take()
                    .unwrap()
                    .send(())
                    .unwrap();
                self.release.lock().await.take().unwrap().await.unwrap();
            }
        }
    }

    fn frames(rx: &mut mpsc::UnboundedReceiver<OutputItem>) -> Vec<Value> {
        std::iter::from_fn(|| rx.try_recv().ok())
            .map(|item| match item {
                OutputItem::Frame(frame) | OutputItem::Negotiate(frame) => {
                    serde_json::from_str(&frame.stringify()).unwrap()
                }
            })
            .collect()
    }

    pub(super) fn fixture() -> (Host, mpsc::UnboundedReceiver<OutputItem>) {
        let cwd = std::env::current_dir().unwrap();
        let model = ara_ai::Model {
            id: "terminal-fixture".into(),
            api: "openai-completions".into(),
            provider: "test".into(),
            base_url: String::new(),
            reasoning: false,
            max_tokens: None,
            context_window: None,
            tokenizer: None,
        };
        let provider = super::super::ProviderFactory::startup(
            reqwest::Client::new(),
            model.clone(),
            super::super::Api::OpenaiCompletions,
            Default::default(),
            false,
            false,
        )
        .unwrap();
        let config = AgentConfig {
            model,
            provider: provider.build(),
            system_prompt: Vec::new(),
            tools: Vec::new(),
            tool_choice: None,
            max_tokens: None,
            temperature: None,
            deadline: None,
            // Exercise real Agent ownership without a network/model call.
            max_model_calls: Some(0),
            hooks: Arc::new(ara_agent::NoHooks),
            interrupt_mode: ara_agent::InterruptMode::Immediate,
        };
        let (tx, rx) = mpsc::unbounded_channel();
        let output = Output(tx);
        let connection = CancellationToken::new();
        let session = Session::new(
            None,
            super::super::ephemeral_header(&cwd, None),
            &[],
            None,
            None,
        )
        .unwrap();
        let bash_target = Arc::new(Mutex::new(BashTarget {
            session: session.clone(),
            destination: BashDestination::Current,
        }));
        let (bash_done_tx, _) = mpsc::unbounded_channel();
        let bash_dispatcher = Arc::new(BashDispatcher {
            current: Mutex::new(bash_target.clone()),
            jobs: Mutex::new(std::collections::HashMap::new()),
            next_id: std::sync::atomic::AtomicU64::new(1),
            done: bash_done_tx,
            output: output.clone(),
            connection: connection.clone(),
            cwd: cwd.clone(),
        });
        let emitter_output = output.clone();
        let emitter: Arc<dyn Fn(WireValue) + Send + Sync> =
            Arc::new(move |frame| emitter_output.send(OutputItem::Frame(frame)));
        let snapshot = Arc::new(RwLock::new(ExecutionSnapshot::from_config(&config)));
        let snapshot_publication =
            ara_cli::append_only_context_mode::SnapshotPublication::new(&snapshot);
        let host = Host {
            #[cfg(feature="test-fixture")]
            cold_publication_fault: Mutex::new(None),
            task_dispatch:None,
        peer_runtime:None,peer_sender:None,peer_wake:None,peer_deferred:Default::default(),
        child_turn:None,
            agent: Agent::new(config.clone(), Vec::new()),
            session: session.clone(),
            config: config.clone(),
            skills: Vec::new(),
            max_time: None,
            output: output.clone(),
            connection: connection.clone(),
            active: None,
            handoff_control: Arc::new(Mutex::new(handoff::HandoffControl::default())),
            quota_control: Arc::new(Mutex::new(quota::QuotaControl::default())),
            model_selection_control: Arc::new(Mutex::new(quota::QuotaControl::default())),
            maintenance: None,
            maintenance_continue: None,
            terminal_recovery: maintenance::TerminalRecoveryState::default(),
            local_context_rebase: None,
            #[cfg(test)]
            fixed_snapcompact_observation: None,
            sessions: SessionFactory {
                peer_messaging_enabled: true,
                agent_context: None,
                dir: None,
                cwd,
                provider,
                args: super::super::Args::parse_from(["ara"]),
                explicit_generation: Default::default(),
                mcp_config: None,
                tool_owner: super::super::CliToolOwner::default(),
                uri_port: None,
                startup: None,
                context_controller: None,
                actual_values_factory: None,
                initial_native_session: None,
                peers:None,peer_root:None,peer_attachment:None,
                native_jobs: Default::default(),
                acp_plan: None,
            },
            tool_bridge: ToolBridge::new(emitter.clone()),
            uri_bridge: UriBridge::new(emitter),
            artifact_router: ara_cli::session_artifacts::ArtifactUriRouter::new(
                session.artifacts.clone(),
                None,
            ),
            host_tools: Vec::new(),
            snapshot,
            snapshot_publication,
            admission_tx: mpsc::unbounded_channel().0,
            route_generation: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            drain_queues: false,
            bash_targets: vec![Arc::downgrade(&bash_target)],
            bash_target,
            bash_dispatcher,
            pending_bash: Vec::new(),
            bash_error: None,
            compaction_policy: AutoCompactionPolicy::isolated(true),
            is_compacting: false,
            auto_compaction_pending: false,
            auto_compaction_checked: None,
            retry_policy: RetryPolicy::isolated(true),
            retry: None,
            fallback: fallback::FallbackState::default(),
            auto: auto::AutoState::default(),
            model_usage_order: Arc::new(
                ara_cli::model_usage_order::SqliteModelUsageOrder::for_path(
                    std::env::temp_dir().join(format!(
                        "ara-mru-fixture-{}.db",
                        session.header["id"].as_str().unwrap()
                    )),
                ),
            ),
            model_cycle_scope: Vec::new(),
            prompt_generation: 0,
            loop_guard_settings: LoopGuardSettings::default(),
            unexpected_stop_mode: UnexpectedStopMode::default(),
            header_continue: None,
            input_closed: false,
            tool_loop: Arc::new(Mutex::new(ToolLoopState::default())),
            tool_loop_settings_dir: None,
        };
        (host, rx)
    }

    fn fresh_actual_values_factory_admission_observations() {
        use ara_rpc::value_heap::{NoRuntime, ValueHeap};
        let (mut host, _) = fixture();
        assert!(host.fresh_actual_session().unwrap().is_none());
        let foundation = ara_cli::native_factory::PreparedNativeSession::prepare().unwrap();
        host.sessions.actual_values_factory = Some(Arc::new({
            let foundation = foundation.clone();
            move || Ok(PreparedActualSession::Native(foundation.clone()))
        }));
        let draft = host.fresh_actual_session().unwrap().unwrap();
        assert!(Arc::ptr_eq(draft.values(), foundation.values()));
        assert!(
            validate_native_session_hooks(
                &host.config,
                &Some(draft.values().clone()),
                &draft.native_session()
            )
            .is_err(),
            "A foundation draft cannot enter serve/adopt before original async tool setup"
        );
        assert!(
            host.session.values.is_none(),
            "Draft preparation must not publish a Session"
        );
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        host.sessions.actual_values_factory = Some(Arc::new({
            let count = count.clone();
            move || {
                count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(PreparedActualSession::FixtureValues(Arc::new(
                    SessionValues::new(ValueHeap::new()?, Arc::new(NoRuntime))?,
                )))
            }
        }));
        let first = host
            .fresh_actual_session()
            .unwrap()
            .unwrap()
            .values()
            .clone();
        let second = host
            .fresh_actual_session()
            .unwrap()
            .unwrap()
            .values()
            .clone();
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_ne!(first.heap().heap_token(), second.heap().heap_token());
        assert!(
            host.session.values.is_none(),
            "Preparing fresh values must not publish a Session"
        );
        let retained = Session::new(
            None,
            host.session.header.clone(),
            &[],
            Some(first.clone()),
            None,
        )
        .unwrap();
        host.bash_target.lock().unwrap().session = retained.clone();
        host.sessions.actual_values_factory = Some(Arc::new({
            let first = first.clone();
            move || Ok(PreparedActualSession::FixtureValues(first.clone()))
        }));
        assert!(
            host.fresh_actual_session().is_err(),
            "An initial realm cannot reuse a retained Bash Session"
        );
        host.session = retained;
        host.sessions.actual_values_factory = None;
        assert!(
            host.fresh_actual_session().is_err(),
            "An actual Session must not silently revert to typed values"
        );
        let alias =
            Arc::new(SessionValues::new(first.heap().clone(), Arc::new(NoRuntime)).unwrap());
        host.sessions.actual_values_factory = Some(Arc::new(move || {
            Ok(PreparedActualSession::FixtureValues(alias.clone()))
        }));
        assert!(
            host.fresh_actual_session().is_err(),
            "A new values Arc is not a new realm"
        );
    }

    // OMP 596f2da B-651c45def5; MIT, copyright (c) Can Bölük.
    // Entry differs: this Host family uses before-prompt maintenance rather than
    // OMP's message_end/agent_end. The prepared-result spy is wider than OMP's
    // compact spy; this case proves orchestration, not renderer/preparation.
    #[tokio::test]
    async fn original_held_usage_frame_rebuild_dead_end_family() {
        original_held_usage_frame_dead_end_observations(false).await;
    }

    // Fixed triggerMaintenance: real MessageEnd persistence, held AgentEnd,
    // owned completed receipt, then the unchanged serial pending consumer.
    // The Completed report is controlled Host evidence, not a live model call.
    #[tokio::test]
    async fn original_post_event_held_usage_frame_dead_end_family() {
        original_held_usage_frame_dead_end_observations(true).await;
    }

    async fn original_held_usage_frame_dead_end_observations(post_event: bool) {
        use ara_agent::compaction::local_reduction::ProtectedToolMatcher;
        use ara_cli::snapcompact::PreparedSnapcompact;
        use ara_session::{NativeSnapcompactArchiveSource, NativeSnapcompactSummary};
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("session.jsonl");
        let preserve = |count: usize| {
            let frames: Vec<_> = (0..count)
                .map(|i| {
                    json!({
                        "data":STANDARD.encode(format!("stale-frame-{i}")),
                        "mimeType":"image/png","cols":4,"rows":2,"chars":8
                    })
                })
                .collect();
            json!({"snapcompact":{"frames":frames,
                "text":format!("HEAD sentinel. {}TAIL sentinel.", "Archived history line. ".repeat(200)),
                "totalChars":4600,"truncatedChars":0}})
        };
        let rows = [
            json!({"type":"session","version":3,"id":"original-held-usage","cwd":dir.path(),"timestamp":"2026-10-02T00:00:00.000Z"}),
            json!({"type":"message","id":"root-user","parentId":null,"timestamp":"2026-10-02T00:00:00.000Z", "message":{"role":"user","content":"hello","timestamp":1}}),
            json!({"type":"compaction","id":"stale-archive","parentId":"root-user","timestamp":"2026-10-02T00:00:00.000Z", "summary":"Archived history onto stale snapcompact frames.", "shortSummary":"stale snapcompact archive","method":"snapcompact", "firstKeptEntryId":"root-user","tokensBefore":150000, "details":{"readFiles":["src/a.ts"],"modifiedFiles":["src/b.ts"]}, "preserveData":preserve(16)}),
        ];
        std::fs::write(
            &file,
            rows.iter()
                .map(|row| format!("{row}\n"))
                .collect::<String>(),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("config.yml"),
            "compaction:\n  methodOrder: [snapcompact, soft]\n  autoContinue: true\n  thresholdTokens: 60000\n",
        )
        .unwrap();
        let (mut host, mut rx) = fixture();
        assert!(host.fixed_snapcompact_observation.is_none());
        host.compaction_policy = AutoCompactionPolicy::load(dir.path()).unwrap();
        host.sessions.args.compact_threshold = 60_000;
        host.sessions.provider.metadata = Some(json!({"input":["text","image"]}));
        host.config.model.id = "claude-sonnet-4-5".into();
        host.config.model.api = "anthropic-messages".into();
        host.config.model.provider = "anthropic".into();
        host.config.model.max_tokens = Some(64_000);
        host.config.model.context_window = Some(200_000.0);
        host.config.system_prompt = vec!["Test".into()];
        host.agent = Agent::new(host.config.clone(), Vec::new());
        let journal = SessionJournal::open(&file).unwrap();
        let original: Vec<_> = journal
            .entries()
            .iter()
            .map(|entry| entry.raw.clone())
            .collect();
        let header = journal.header().clone();
        host.session = Session::new(Some(journal), header, &[], None, None).unwrap();
        host.bash_target.lock().unwrap().session = host.session.clone();
        host.artifact_router.bind(host.session.artifacts.clone());
        {
            let session = host.session.clone();
            let journal = session.journal.lock().await;
            host.adopt_route_context(&journal).await.unwrap();
            let snapshot = journal.native_snapcompact_snapshot().unwrap();
            assert_eq!(journal.branch().last().unwrap().kind, "compaction");
            assert!(
                ara_cli::snapcompact::prepare_snapcompact(
                    &snapshot,
                    &host.config.model,
                    host.sessions.args.compact_keep_tokens,
                    150_000,
                    &ara_cli::snapcompact::SnapcompactPolicy {
                        shape: "auto".into(),
                        reserve_tokens: Some(16_384.0),
                        non_message_tokens: host.non_message_tokens(),
                        pending_tokens: 0,
                        deployment: None,
                    }
                )
                .unwrap()
                .is_none(),
                "original native preparation must organically be absent"
            );
        }
        let raw_preserve =
            serde_json::value::RawValue::from_string(preserve(4).to_string()).unwrap();
        let result = ara_snapcompact::CompactionResultUtf16 {
            summary: "Rebuilt archive at a smaller frame budget.".into(),
            short_summary: Some("rebuilt snapcompact archive".into()),
            first_kept_entry_id: "root-user".into(),
            tokens_before: 150_000.0,
            details: Some(ara_snapcompact::CompactionDetails {
                read_files: Vec::new(),
                modified_files: Vec::new(),
            }),
            preserve_data: Some(raw_preserve.clone()),
        };
        let prepared = PreparedSnapcompact {
            summary: NativeSnapcompactSummary {
                summary: result.summary.clone(),
                short_summary: result.short_summary.clone(),
                preserve_data: Some(preserve(4)),
                preserve_source: Some(
                    NativeSnapcompactArchiveSource::from_raw(&raw_preserve).unwrap(),
                ),
                read_files: Vec::new(),
                modified_files: Vec::new(),
            },
            result,
            window_source_entry_ids: Vec::new(),
            tokens_after: 190_000,
        };
        let observation = Arc::new(Mutex::new(FixedSnapcompactObservation {
            usage_tokens: 190_000,
            frame_rescue_result: Some(prepared),
            frame_rescue_returns: 0,
            shake_calls: Vec::new(),
        }));
        host.fixed_snapcompact_observation = Some(observation.clone());
        assert_eq!(
            host.context_tokens(&host.agent.raw_messages().await, &[])
                .await
                .unwrap(),
            190_000
        );
        if post_event {
            let mut done =
                AssistantMessage::empty("anthropic-messages", "anthropic", "claude-sonnet-4-5");
            done.content.push(ara_ai::AssistantBlock::text("Done."));
            done.usage = ara_ai::Usage {
                input: Some(190_000),
                output: Some(1_000),
                cache_read: Some(0),
                cache_write: Some(0),
                total_tokens: Some(191_000),
                reasoning_tokens: None,
                cost: Some(ara_ai::Cost {
                    input: 0.0,
                    output: 0.0,
                    cache_read: 0.0,
                    cache_write: 0.0,
                    total: 0.0,
                }),
            };
            let done: AgentMessage = Message::Assistant(done).into();
            host.agent.append_idle_raw_message(done.clone()).unwrap();
            let cancel = CancellationToken::new();
            let sink = Arc::new(RunSink {
                session: host.session.clone(),
                observed_usage: None,
                output: host.output.clone(),
                cancel: cancel.clone(),
                connection: host.connection.clone(),
                terminal: Mutex::new(None),
                messages: Mutex::new(Vec::new()),
                entries: Mutex::new(Vec::new()),
                header_guard: Mutex::new(GeminiHeaderGuard::new(false, 0)),
            });
            sink.emit(AgentEvent::MessageEnd {
                message: done.clone(),
            })
            .await;
            sink.emit(AgentEvent::AgentEnd {
                messages: vec![done.clone()],
            })
            .await;
            assert!(!host.auto_compaction_pending);
            assert!(sink.terminal.lock().unwrap().is_some());
            {
                let journal = host.session.journal.lock().await;
                let tail = journal.branch().last().unwrap().raw.clone();
                assert_eq!(tail["type"], "message");
                assert_eq!(tail["message"]["content"][0]["text"], "Done.");
                assert_eq!(tail["message"]["usage"]["input"], 190_000);
                assert_eq!(tail["message"]["usage"]["output"], 1_000);
                assert_eq!(tail["message"]["usage"]["totalTokens"], 191_000);
                let snapshot = journal.native_snapcompact_snapshot().unwrap();
                assert!(
                    ara_cli::snapcompact::prepare_snapcompact(
                        &snapshot,
                        &host.config.model,
                        host.sessions.args.compact_keep_tokens,
                        150_000,
                        &ara_cli::snapcompact::SnapcompactPolicy {
                            shape: "auto".into(),
                            reserve_tokens: Some(16_384.0),
                            non_message_tokens: host.non_message_tokens(),
                            pending_tokens: 0,
                            deployment: None,
                        }
                    )
                    .unwrap()
                    .is_none(),
                    "original post-event preparation must remain organically absent with persisted Done"
                );
            }
            let report = RunReport {
                messages: vec![done],
                end: RunEnd::Completed,
                stop_cause: None,
            };
            let task = tokio::spawn(async move { Ok(Some(report)) });
            let mut active = ActiveRun {
                child_turn: None,
                native: None,
                cancel,
                task,
                command: Command::new(wire(json!({"type":"prompt","id":"original-post-event"}))),
                sink,
            };
            let result = (&mut active.task).await;
            Box::pin(host.completed(active, result)).await;
            assert!(
                host.auto_compaction_pending,
                "the real Completed receipt must arm pending maintenance"
            );
            assert!(!host.drain_pending_auto_compaction(false).await);
            assert!(!host.auto_compaction_pending);
            assert!(!host.drain_queues);
        } else {
            assert!(!host.maybe_auto_compact_before_start(&[], false).await);
        }
        assert!(host.active.is_none());
        assert!(host.local_context_rebase.is_none());
        assert_eq!(
            host.context_tokens(&host.agent.raw_messages().await, &[])
                .await
                .unwrap(),
            190_000
        );
        let observed = observation.lock().unwrap();
        assert_eq!(observed.frame_rescue_returns, 1);
        assert!(observed.frame_rescue_result.is_none());
        assert_eq!(observed.shake_calls.len(), 1);
        let (rescue, config) = &observed.shake_calls[0];
        assert!(*rescue);
        assert_eq!(config.protect_tokens, 0.0);
        assert_eq!(config.min_savings, 0.0);
        assert_eq!(config.fence_min_tokens, 400.0);
        assert_eq!(config.keep_boundary_id, None);
        assert_eq!(
            config.protected_tools,
            vec![
                ProtectedToolMatcher::ToolName("skill".into()),
                ProtectedToolMatcher::SkillRead,
                ProtectedToolMatcher::ArtifactRecovery
            ]
        );
        drop(observed);
        let events = frames(&mut rx);
        let warnings: Vec<_> = events
            .iter()
            .filter(|frame| {
                frame["type"] == "notice"
                    && frame["source"] == "compaction"
                    && frame["message"].as_str().is_some_and(|s| {
                        s.contains("Compaction freed too little context to make progress")
                    })
            })
            .collect();
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0]["level"], "warning");
        assert!(
            warnings[0]["message"]
                .as_str()
                .unwrap()
                .contains("reduce archived image frames (")
        );
        let ends: Vec<_> = events
            .iter()
            .filter(|frame| frame["type"] == "auto_compaction_end")
            .collect();
        assert_eq!(ends.len(), 1);
        assert_eq!(ends[0]["willRetry"], false);
        assert!(
            ends[0].get("errorMessage").is_none(),
            "fixed no-preparation is an explained pause, not a lifecycle error"
        );
        assert_eq!(ends[0]["skipped"], false);
        assert_eq!(ends[0]["action"], "snapcompact");
        assert_eq!(ends[0]["aborted"], false);
        assert_eq!(
            ends[0]["result"]["summary"],
            "Rebuilt archive at a smaller frame budget."
        );
        assert!(ends[0]["result"].get("preserveData").is_none());
        let starts: Vec<_> = events
            .iter()
            .filter(|frame| frame["type"] == "auto_compaction_start")
            .collect();
        assert_eq!(starts.len(), 1);
        assert_eq!(starts[0]["action"], "snapcompact");
        let end_index = events
            .iter()
            .position(|frame| frame["type"] == "auto_compaction_end")
            .unwrap();
        let warning_index = events
            .iter()
            .position(|frame| frame["type"] == "notice" && frame["level"] == "warning")
            .unwrap();
        assert!(end_index < warning_index);
        if post_event {
            let message_index = events
                .iter()
                .position(|frame| frame["type"] == "message_end")
                .unwrap();
            let terminal_index = events
                .iter()
                .position(|frame| frame["type"] == "agent_end")
                .unwrap();
            let start_index = events
                .iter()
                .position(|frame| frame["type"] == "auto_compaction_start")
                .unwrap();
            assert!(message_index < terminal_index && terminal_index < start_index);
            assert_eq!(
                events
                    .iter()
                    .filter(|frame| frame["type"] == "agent_end")
                    .count(),
                1
            );
        }
        let journal = host.session.journal.lock().await;
        assert_eq!(
            &journal.entries()[..original.len()]
                .iter()
                .map(|entry| entry.raw.clone())
                .collect::<Vec<_>>(),
            &original
        );
        let archives: Vec<_> = journal
            .branch()
            .into_iter()
            .filter(|entry| entry.kind == "compaction")
            .collect();
        assert_eq!(archives.len(), 2);
        let active = archives.last().unwrap();
        assert_eq!(
            active.raw["preserveData"]["snapcompact"]["frames"]
                .as_array()
                .unwrap()
                .len(),
            4
        );
        assert_eq!(
            active.raw["summary"],
            "Rebuilt archive at a smaller frame budget."
        );
        assert!(
            active.raw["warning"]
                .as_str()
                .unwrap()
                .contains("Compaction freed too little context to make progress")
        );
        assert!(
            active.raw["warning"]
                .as_str()
                .unwrap()
                .contains("reduce archived image frames (")
        );
        if post_event {
            let done: Vec<_> = journal
                .branch()
                .into_iter()
                .filter(|entry| entry.raw["message"]["role"] == "assistant")
                .collect();
            assert_eq!(done.len(), 1);
            assert_eq!(done[0].raw["message"]["content"][0]["text"], "Done.");
            assert_eq!(journal.branch().last().unwrap().id, active.id);
        }
    }

    #[tokio::test]
    async fn public_model_selection_identity_retains_catalog_id_and_execution_receipts() {
        fresh_actual_values_factory_admission_observations();
        let (mut host, _) = fixture();
        host.config.model.id = "execution-wire-id".into();
        host.sessions.provider.metadata = Some(json!({"catalogModelId":"selected-catalog-id"}));
        let state = host.state().await;
        assert_eq!(state["model"]["id"], "selected-catalog-id");
        assert_eq!(host.config.model.id, "execution-wire-id");
        host.sessions.provider.metadata = None;
        assert_eq!(host.state().await["model"]["id"], "execution-wire-id");
    }

    #[tokio::test]
    async fn native_incomplete_cap_progress_and_foreign_terminal_host_module() {
        for case in [
            "reasoning-cap",
            "visible-progress",
            "old-responses",
            "foreign-length",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let (mut host, _) = fixture();
            let mut journal = SessionJournal::create(dir.path(), dir.path()).unwrap();
            let user = Message::User(UserMessage::text("pending original task"));
            journal.append_message(&user).unwrap();
            let mut assistant = AssistantMessage::empty(
                &host.config.model.api,
                &host.config.model.provider,
                &host.config.model.id,
            );
            assistant.stop_reason = ara_ai::StopReason::Length;
            if case == "reasoning-cap" {
                assistant
                    .content
                    .push(ara_ai::AssistantBlock::Thinking(ara_ai::ThinkingContent {
                        thinking: "reasoning without deliverable".into(),
                        thinking_signature: None,
                    }));
            }
            if case == "visible-progress" {
                assistant.content.push(ara_ai::AssistantBlock::text(
                    "actionable partial deliverable",
                ));
            }
            if case == "old-responses" {
                host.config.model.api = "openai-responses".into();
                assistant.api = host.config.model.api.clone();
            }
            if case == "foreign-length" {
                assistant.model = "foreign-model".into();
            }
            let message = Message::Assistant(assistant.clone());
            let entry = journal.append_message(&message).unwrap();
            let file = journal.path().to_path_buf();
            let header = journal.header().clone();
            host.session = Session::new(Some(journal), header, &[], None, None).unwrap();
            let messages = {
                let journal = host.session.journal.try_lock().unwrap();
                host.route_raw_context(&journal).unwrap()
            };
            host.agent.replace_idle_raw_messages(messages).unwrap();
            host.terminal_recovery.incomplete_attempts = 3;
            let cancel = CancellationToken::new();
            let sink = Arc::new(RunSink {
                session: host.session.clone(),
                observed_usage: None,
                output: host.output.clone(),
                cancel: cancel.clone(),
                connection: host.connection.clone(),
                terminal: Mutex::new(None),
                messages: Mutex::new(Vec::new()),
                entries: Mutex::new(vec![(entry, message)]),
                header_guard: Mutex::new(GeminiHeaderGuard::new(false, 0)),
            });
            let active = ActiveRun {
                child_turn: None,
                native: None,
                cancel,
                task: tokio::spawn(async { Ok(None) }),
                command: Command::new(wire(json!({"type":"prompt"}))),
                sink,
            };
            let outcome = Box::pin(host.begin_overflow_recovery(&active, &assistant))
                .await
                .unwrap();
            let loaded = SessionJournal::open(&file).unwrap();
            loaded.projected_compaction_snapshot().unwrap();
            match case {
                "reasoning-cap" => {
                    assert!(outcome.automatic_continuation_blocked && outcome.history_rewritten);
                    assert_eq!(host.terminal_recovery.incomplete_attempts, 0);
                    assert!(
                        loaded
                            .model_context()
                            .unwrap()
                            .iter()
                            .all(|m| m.as_assistant().is_none())
                    );
                }
                "visible-progress" => {
                    assert!(!outcome.history_rewritten);
                    assert_eq!(host.terminal_recovery.incomplete_attempts, 1);
                    assert_eq!(
                        loaded
                            .model_context()
                            .unwrap()
                            .last()
                            .unwrap()
                            .as_assistant()
                            .unwrap()
                            .text(),
                        "actionable partial deliverable"
                    );
                }
                _ => {
                    assert!(!outcome.continuation_scheduled && !outcome.history_rewritten);
                    assert_eq!(host.terminal_recovery.incomplete_attempts, 3);
                }
            }
            assert!(!host.connection.is_cancelled());
        }
    }

    #[tokio::test]
    async fn bash_dispatch_starts_child_while_serial_abort_waits_for_run_settlement() {
        let dir = tempfile::tempdir().unwrap();
        let (mut host, mut frames_rx) = fixture();
        let (done_tx, mut done_rx) = mpsc::unbounded_channel();
        let dispatcher = Arc::new(BashDispatcher {
            current: Mutex::new(host.bash_target.clone()),
            jobs: Mutex::new(std::collections::HashMap::new()),
            next_id: std::sync::atomic::AtomicU64::new(1),
            done: done_tx,
            output: host.output.clone(),
            connection: host.connection.clone(),
            cwd: dir.path().into(),
        });
        host.bash_dispatcher = dispatcher.clone();
        let cancel = CancellationToken::new();
        let sink = Arc::new(RunSink {
            session: host.session.clone(),
            observed_usage: None,
            output: host.output.clone(),
            cancel: cancel.clone(),
            connection: host.connection.clone(),
            terminal: Mutex::new(None),
            messages: Mutex::new(Vec::new()),
            entries: Mutex::new(Vec::new()),
            header_guard: Mutex::new(GeminiHeaderGuard::new(false, 0)),
        });
        let (reached_tx, reached_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let gate = Arc::new(TerminalGate {
            inner: sink.clone(),
            reached: Mutex::new(Some(reached_tx)),
            release: tokio::sync::Mutex::new(Some(release_rx)),
        });
        let agent = host.agent.clone();
        let config = host.config.clone();
        let run_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            agent
                .prompt_inputs_with_config(
                    vec![
                        Message::User(UserMessage {
                            content: UserContent::Text("gated".into()),
                            synthetic: None,
                            timestamp: 0,
                            provider_payload: None,
                            provider_payload_origin: None,
                        })
                        .into(),
                    ],
                    config,
                    run_cancel,
                    gate,
                )
                .await
                .map(Some)
        });
        host.active = Some(ActiveRun {
            child_turn: None,
            native: None,
            cancel,
            task,
            command: Command::new(wire(json!({"type":"prompt"}))),
            sink,
        });
        tokio::time::timeout(Duration::from_secs(3), reached_rx)
            .await
            .unwrap()
            .unwrap();
        let serial = tokio::spawn(async move {
            host.abort().await;
            host
        });
        dispatcher.dispatch(Command::new(wire(
            json!({"type":"bash","id":"overtake", "command":"printf admitted > admitted.txt"}),
        )));
        let completion = tokio::time::timeout(Duration::from_secs(4), done_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("admitted.txt")).unwrap(),
            "admitted"
        );
        assert!(
            !serial.is_finished(),
            "Bash must run before serial Run settlement is released"
        );
        release_tx.send(()).unwrap();
        let mut host = tokio::time::timeout(Duration::from_secs(3), serial)
            .await
            .unwrap()
            .unwrap();
        host.completed_bash(completion).await;
        let output = frames(&mut frames_rx);
        assert!(
            output
                .iter()
                .any(|frame| frame["id"] == "overtake" && frame["success"] == true)
        );
        assert_eq!(
            host.session
                .messages
                .lock()
                .unwrap()
                .iter()
                .filter(|message| public_wire_is(message, "role", "bashExecution"))
                .count(),
            1
        );
        assert!(host.bash_dispatcher.is_empty());
    }

    #[tokio::test]
    async fn bash_receipt_write_failure_preserves_process_effect_and_blocks_next_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let (mut host, mut output_rx) = fixture();
        let mut journal = SessionJournal::create(dir.path(), dir.path()).unwrap();
        journal.materialize().unwrap();
        let path = journal.path().to_path_buf();
        let header = journal.header().clone();
        host.session = Session::new(Some(journal), header, &[], None, None).unwrap();
        let target = Arc::new(Mutex::new(BashTarget {
            session: host.session.clone(),
            destination: BashDestination::Current,
        }));
        let (done_tx, mut done_rx) = mpsc::unbounded_channel();
        host.bash_target = target.clone();
        host.bash_targets = vec![Arc::downgrade(&target)];
        host.bash_dispatcher = Arc::new(BashDispatcher {
            current: Mutex::new(target),
            jobs: Mutex::new(std::collections::HashMap::new()),
            next_id: std::sync::atomic::AtomicU64::new(1),
            done: done_tx,
            output: host.output.clone(),
            connection: host.connection.clone(),
            cwd: dir.path().into(),
        });
        host.bash_dispatcher.dispatch(Command::new(wire(
            json!({"type":"bash","id":"disk-fault", "command":"printf happened > effect.txt"}),
        )));
        let completion = tokio::time::timeout(Duration::from_secs(4), done_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("effect.txt")).unwrap(),
            "happened"
        );
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        host.completed_bash(completion).await;
        assert!(host.connection.is_cancelled());
        assert!(host.session.persistence_error.lock().unwrap().is_some());
        assert!(
            host.bash_error
                .as_deref()
                .unwrap()
                .contains("effects may have occurred")
        );
        assert!(
            host.start(None, Command::new(wire(json!({"type":"prompt"}))))
                .await
                .is_err()
        );
        assert!(!host.agent.is_busy());
        assert!(
            frames(&mut output_rx)
                .iter()
                .any(|frame| frame["id"] == "disk-fault" && frame["success"] == false)
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("effect.txt")).unwrap(),
            "happened"
        );
    }

    #[test]
    fn metadata_keeps_hidden_skills_and_falls_back_without_source_fields() {
        let (mut host, _) = fixture();
        host.skills.push(LoadedSkill {
            name: "隐藏🦀".into(),
            description: String::new(),
            file_path: PathBuf::from("private/SKILL.md"),
            base_dir: PathBuf::from("private"),
            source: "fixture:project".into(),
            hide: true,
            meta: ara_discovery::SourceMeta::new(
                "fixture",
                std::path::Path::new("private"),
                ara_discovery::Level::Project,
            ),
        });
        assert_eq!(
            host.available_commands(),
            vec![json!({"name":"skill:隐藏🦀",
            "description":"Run 隐藏🦀 skill","input":{"hint":"arguments"},"source":"skill"})]
        );
    }

    #[test]
    fn paging_error_keeps_unpaired_utf16_correlation_id_and_code() {
        let (host, mut rx) = fixture();
        let command = Command::new(
            WireValue::parse(r#"{"type":"get_messages_page","id":"\ud800"}"#).unwrap(),
        );
        host.output.response_wire(
            &command,
            None,
            Some("RPC message cursor is stale".into()),
            Some("stale_cursor"),
        );
        let OutputItem::Frame(frame) = rx.try_recv().unwrap() else {
            panic!("ordinary response")
        };
        assert_eq!(frame.get("id"), command.id.as_ref());
        assert_eq!(
            frame.get("code"),
            Some(&WireValue::String("stale_cursor".into()))
        );
        assert!(frame.get("data").is_none());
    }

    #[test]
    fn stats_preserve_unknown_buckets_and_custom_identity() {
        let messages = vec![
            json!({"role":"user"}),
            json!({"role":"custom","customType":"skill-prompt"}),
            json!({"role":"assistant","content":[{"type":"toolCall"}],
                "usage":{"input":5,"output":2,"cacheRead":0,"totalTokens":7,"cost":{"total":0.5}}}),
            json!({"role":"toolResult"}),
            json!({"role":"assistant","content":[],"usage":{"input":3,"totalTokens":3}}),
        ];
        let stats = session_stats(&messages);
        assert_eq!(stats["userMessages"], 1);
        assert_eq!(stats["assistantMessages"], 2);
        assert_eq!(stats["toolCalls"], 1);
        assert_eq!(stats["toolResults"], 1);
        assert_eq!(stats["totalMessages"], 5);
        assert_eq!(
            stats["tokens"],
            json!({"input":8,"output":null,"reasoning":null,
            "cacheRead":null,"cacheWrite":null,"total":10})
        );
        assert!(stats["cost"].is_null());
        assert!(stats["premiumRequests"].is_null());
        assert_eq!(session_stats(&[])["cost"], 0.0);
        let classifier = json!({"input":4,"output":1,"totalTokens":5,"cost":{"total":0.25}});
        let with_classifier =
            session_stats_with_model_usage(&messages, std::slice::from_ref(&classifier));
        assert_eq!(with_classifier["tokens"]["input"], 12);
        assert_eq!(with_classifier["tokens"]["total"], 15);
        assert!(with_classifier["tokens"]["output"].is_null());
        for field in [
            "userMessages",
            "assistantMessages",
            "toolCalls",
            "toolResults",
            "totalMessages",
        ] {
            assert_eq!(with_classifier[field], stats[field]);
        }
        let legacy_projector = ara_cli::agent_message_projection::AgentMessageProjector::default();
        let legacy_event = AgentEvent::ToolExecutionEnd {
            tool_call_id: "legacy-u64".into(),
            tool_name: "read".into(),
            result: ara_agent::ToolOutput::text("scalar legacy")
                .with_details(json!({"legacyCounter":9007199254740993u64})),
            is_error: false,
        };
        for include_snapshot in [false, true] {
            let expected = if include_snapshot {
                legacy_event.full().unwrap()
            } else {
                legacy_event.printable().unwrap()
            };
            assert_eq!(
                legacy_projector
                    .public_event(&legacy_event, include_snapshot)
                    .unwrap(),
                expected,
                "unbound legacy public printer retains valid serde u64 exactly"
            );
            let expected_wire = WireValue::parse(&expected.to_string()).unwrap();
            assert!(
                legacy_projector
                    .public_event_wire(&legacy_event, include_snapshot)
                    .unwrap()
                    .deep_equal(&expected_wire),
                "RPC retains its existing explicit legacy number wire boundary"
            );
        }
        let mut exact = messages.iter().cloned().map(wire).collect::<Vec<_>>();
        assert_eq!(
            public_wire_session_stats(&exact, &[]),
            stats,
            "Wire cache observes the original statistics fields"
        );
        assert_eq!(
            public_wire_session_stats(&exact, std::slice::from_ref(&classifier)),
            with_classifier
        );
        let opaque = WireValue::String(ara_rpc::WireString::from_units(vec![0xd800, 0xd83d]));
        exact[2].insert(
            "content",
            WireValue::Array(vec![
                wire(json!({"type":"toolCall"})),
                WireValue::object(vec![
                    ("type", WireValue::String("text".into())),
                    ("text", opaque.clone()),
                ]),
            ]),
        );
        exact[2].insert("businessExtension", opaque);
        assert_eq!(
            public_wire_session_stats(&exact, &[]),
            stats,
            "opaque business UTF16 is not a statistics bridge input"
        );
        let text_message = WireValue::object(vec![
            ("role", WireValue::String("assistant".into())),
            (
                "content",
                WireValue::Array(vec![
                    WireValue::object(vec![
                        ("type", WireValue::String("text".into())),
                        (
                            "text",
                            WireValue::String(ara_rpc::WireString::from_units(vec![
                                0xfeff, 32, 0xd800,
                            ])),
                        ),
                    ]),
                    WireValue::object(vec![
                        ("type", WireValue::String("text".into())),
                        (
                            "text",
                            WireValue::String(ara_rpc::WireString::from_units(vec![
                                0xd83d, 32, 0xfeff,
                            ])),
                        ),
                    ]),
                ]),
            ),
        ]);
        assert_eq!(
            last_assistant_wire_text(std::slice::from_ref(&text_message))
                .unwrap()
                .units(),
            &[0xd800, 0xd83d]
        );
        assert_eq!(
            session_stats_with_model_usage(&[], &[classifier])["cost"],
            0.25
        );
        assert!(session_stats_with_model_usage(&[], &[json!({})])["cost"].is_null());
    }

    // Existing coarse RPC entrance also exercises exact archive publication,
    // real ordered transport, Native observation and bounded output failures.
    async fn exact_archive_public_transport() {
        use tokio::io::AsyncReadExt;
        let directory = tempfile::tempdir().unwrap();
        let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
        let mut old = Vec::new();
        old.push(
            journal
                .append_message(&Message::User(ara_ai::UserMessage::text("old user")))
                .unwrap(),
        );
        let mut answer = AssistantMessage::empty("openai-completions", "fixture", "m");
        answer
            .content
            .push(ara_ai::AssistantBlock::text("old answer"));
        old.push(journal.append_message(&Message::Assistant(answer)).unwrap());
        let kept = journal
            .append_message(&Message::User(ara_ai::UserMessage::text("kept")))
            .unwrap();
        let raw = serde_json::value::RawValue::from_string(
            r#"{"extraMeta":"\udfff","snapcompact":{
            "frames":[],"totalChars":9,"truncatedChars":0,"text":"archive \ud800"}}"#
                .into(),
        )
        .unwrap();
        let summary = ara_session::NativeSnapcompactSummary {
            summary: "Exact archive".into(),
            short_summary: Some(String::new()),
            preserve_data: None,
            preserve_source: Some(
                ara_session::NativeSnapcompactArchiveSource::from_raw(&raw).unwrap(),
            ),
            read_files: Vec::new(),
            modified_files: Vec::new(),
        };
        let prepared = ara_cli::snapcompact::PreparedSnapcompact {
            result: ara_snapcompact::CompactionResultUtf16 {
                summary: summary.summary.clone(),
                short_summary: summary.short_summary.clone(),
                first_kept_entry_id: kept.clone(),
                tokens_before: 200.0,
                details: None,
                preserve_data: Some(raw),
            },
            summary,
            window_source_entry_ids: old.clone(),
            tokens_after: 20,
        };
        let mut receipt = snapcompact_public_response(&prepared).unwrap();
        assert!(
            receipt
                .get("preserveData")
                .unwrap()
                .get("snapcompact")
                .is_none()
        );
        journal.materialize().unwrap();
        let snapshot = journal.native_snapcompact_snapshot().unwrap();
        let published = journal
            .commit_native_entry_snapcompact(&snapshot, &prepared.summary, &kept, &old, 200)
            .unwrap();
        receipt.insert("entryId", wire(json!(published)));
        let reopened = SessionJournal::open(journal.path()).unwrap();
        assert_eq!(reopened.report.malformed_records, 0);
        assert!(
            reopened
                .native_snapcompact_snapshot()
                .unwrap()
                .projection
                .previous_summary
                .unwrap()
                .has_archive()
        );
        // The existing public message view is intentionally a summary view;
        // invoking it cannot invalidate the journal's exact archive owner.
        assert!(!Session::public_messages(&reopened).unwrap().is_empty());
        let exact = receipt
            .get("preserveData")
            .unwrap()
            .get("extraMeta")
            .unwrap()
            .as_string()
            .unwrap();
        assert_eq!(exact.units(), &[0xdfff]);
        let (tx, rx) = mpsc::unbounded_channel();
        let output = Output(tx);
        let command =
            Command::new(WireValue::parse(r#"{"type":"compact","id":"\ud800"}"#).unwrap());
        output.response_wire(&command, Some(receipt.clone()), None, None);
        drop(output);
        let (writer, mut reader) = tokio::io::duplex(16384);
        let cancel = CancellationToken::new();
        let task = tokio::spawn(write_output(writer, rx, cancel.clone()));
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await.unwrap();
        task.await.unwrap().unwrap();
        let mut decoder = ara_rpc::RpcFrameDecoder::new();
        let mut decoded = None;
        for line in std::str::from_utf8(&bytes).unwrap().lines() {
            if let Some(frame) = decoder.push(WireValue::parse(line).unwrap()).unwrap() {
                decoded = Some(frame);
            }
        }
        let decoded = decoded.unwrap();
        assert_eq!(decoded.get("id"), command.id.as_ref());
        assert_eq!(decoded.get("data"), Some(&receipt));
        assert!(!cancel.is_cancelled());
        let event = WireValue::object(vec![
            ("type", wire(json!("auto_compaction_end"))),
            ("result", receipt),
        ]);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink_seen = seen.clone();
        let io = Arc::new(NativeOutput {
            stop_after_child_shutdown: false,
            pending: Arc::new(Mutex::new(std::collections::HashMap::new())),
            frames: Arc::new(move |frame| sink_seen.lock().unwrap().push(frame)),
            connection: CancellationToken::new(),
            failure: Arc::new(Mutex::new(None)),
            cancel_receipt: Arc::new(NativeCancelReceipt::default()),
        });
        let (tx, rx) = mpsc::unbounded_channel();
        tx.send(OutputItem::Frame(event.clone())).unwrap();
        drop(tx);
        write_native_output(rx, io.clone()).await.unwrap();
        assert_eq!(seen.lock().unwrap().as_slice(), &[event]);
        assert!(!io.connection.is_cancelled());
        // A typed requested-data projection failure replies with an error and
        // cancels the connection; the caller cannot hang or infer success.
        let (reply, received) = tokio::sync::oneshot::channel();
        io.pending.lock().unwrap().insert("typed".into(), reply);
        let (tx, rx) = mpsc::unbounded_channel();
        tx.send(OutputItem::Frame(
            WireValue::parse(
                r#"{"type":"response","id":"typed","success":true,"data":{"opaque":"\ud800"}}"#,
            )
            .unwrap(),
        ))
        .unwrap();
        drop(tx);
        assert!(write_native_output(rx, io.clone()).await.is_err());
        assert!(received.await.unwrap().is_err());
        assert!(io.connection.is_cancelled());
        let (writer, reader) = tokio::io::duplex(64);
        drop(reader);
        let (tx, rx) = mpsc::unbounded_channel();
        tx.send(OutputItem::Frame(wire(json!({"type":"notice"}))))
            .unwrap();
        drop(tx);
        let cancel = CancellationToken::new();
        assert!(write_output(writer, rx, cancel.clone()).await.is_err());
        assert!(cancel.is_cancelled());
        eprintln!(
            "RPC exact archive: publish/reopen, raw stripped receipt, ordered writer/decoder, Native exact sink and typed/output fault cancellation observed"
        );
    }

    // Same coarse RPC ownership entrance: the entered native effect survives
    // graph-driver panic, and no public terminal can overtake its owned join.
    async fn native_driver_panic_effect_ownership() {
        let directory = tempfile::tempdir().unwrap();
        let prepared = crate::native_host_await_observations::prepared(directory.path());
        let weak = Arc::downgrade(&prepared);
        let cancel = CancellationToken::new();
        let driver =
            ara_cli::native_run::NativeRun::begin(prepared.clone(), cancel.clone()).unwrap();
        let lease = driver.lease();
        let worker_cancel = cancel.child_token();
        let effect_cancel = worker_cancel.clone();
        let (entered_tx, entered_rx) = oneshot::channel();
        let (cancelled_tx, cancelled_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        lease
            .effects()
            .reserve()
            .unwrap()
            .enter(
                worker_cancel,
                Box::pin(async move {
                    let _ = entered_tx.send(());
                    effect_cancel.cancelled().await;
                    let _ = cancelled_tx.send(());
                    let _ = release_rx.await;
                    Ok(ara_agent::ToolOutput::text("owned effect settled"))
                }),
            )
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), entered_rx)
            .await
            .unwrap()
            .unwrap();
        let (mut host, mut rx) = fixture();
        let sink = Arc::new(RunSink {
            session: host.session.clone(),
            observed_usage: None,
            output: host.output.clone(),
            cancel: cancel.clone(),
            connection: host.connection.clone(),
            terminal: Mutex::new(Some(wire(json!({"type":"agent_end","messages":[]})))),
            messages: Mutex::new(Vec::new()),
            entries: Mutex::new(Vec::new()),
            header_guard: Mutex::new(GeminiHeaderGuard::new(false, 0)),
        });
        let task = tokio::spawn(async move {
            let _driver = driver;
            panic!("controlled Native graph-driver panic");
            #[allow(unreachable_code)]
            Ok(None)
        });
        let mut active = ActiveRun {
            child_turn: None,
            native: Some(lease.clone()),
            cancel,
            task,
            command: Command::new(wire(json!({"type":"prompt","id":"native-panic"}))),
            sink,
        };
        drop(prepared);
        let result = (&mut active.task).await;
        assert!(result.is_err());
        tokio::time::timeout(Duration::from_secs(3), cancelled_rx)
            .await
            .unwrap()
            .unwrap();
        assert!(
            weak.upgrade().is_some(),
            "ActiveRun must retain complete Prepared after driver panic"
        );
        drop(lease);
        let mut done = tokio::spawn(async move {
            host.completed(active, result).await;
            host
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut done)
                .await
                .is_err(),
            "completed must remain pending while the entered effect is held"
        );
        assert!(
            rx.try_recv().is_err(),
            "public terminal cannot precede entered-effect settlement"
        );
        assert!(weak.upgrade().is_some());
        release_tx.send(()).unwrap();
        let _host = tokio::time::timeout(Duration::from_secs(3), done)
            .await
            .unwrap()
            .unwrap();
        let emitted = frames(&mut rx);
        assert_eq!(emitted[0]["type"], "agent_end");
        assert_eq!(
            emitted.len(),
            2,
            "one terminal and one original prompt failure response"
        );
        let response = &emitted[1];
        assert_eq!(response["type"], "response");
        assert_eq!(response["command"], "prompt");
        assert_eq!(response["id"], "native-panic");
        assert_eq!(response["success"], false);
        let error = response["error"].as_str().unwrap();
        assert!(error.contains("Run failed"));
        assert!(
            error.contains("Native entered effects have unknown outcomes:")
                && error.contains("Host effect 1:"),
            "panic response must retain the independent unknown-effect receipt: {error}"
        );
        assert!(
            weak.upgrade().is_none(),
            "complete Prepared may release only after effect join"
        );
        eprintln!(
            "Native RPC ownership: graph panic, complete Prepared retained, entered-effect join before terminal/early recovery executed"
        );
    }

    #[tokio::test]
    async fn cancelled_before_entry_keeps_origin_and_exact_skip_id() {
        exact_archive_public_transport().await;
        native_driver_panic_effect_ownership().await;
        let (mut host, mut rx) = fixture();
        // A lone UTF-16 surrogate exercises IDs which serde_json cannot
        // represent. No await before abort: the spawned admission is cancelled.
        let command = Command::new(WireValue::parse(r#"{"type":"prompt","id":"\ud800"}"#).unwrap());
        let id = command.id.clone().unwrap();
        let origin = host.session.clone();
        host.start(
            Some(skill_input(UserSkillPrompt::new(
                UserContent::Text("not invoked".into()),
                None,
            ))),
            command,
        )
        .await
        .unwrap();
        host.abort().await;
        assert!(host.agent.messages().await.unwrap().is_empty());
        assert!(origin.messages.lock().unwrap().is_empty());
        assert!(!host.agent.is_busy());
        let item = rx.try_recv().unwrap();
        let OutputItem::Frame(frame) = item else {
            panic!("unexpected negotiation")
        };
        assert_eq!(frame.get("type"), Some(&wire(json!("prompt_result"))));
        assert_eq!(frame.get("agentInvoked"), Some(&WireValue::Bool(false)));
        assert_eq!(frame.get("id"), Some(&id));
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn custom_receipt_failure_cancels_before_provider_and_tool_effects() {
        let dir = tempfile::tempdir().unwrap();
        let mut journal = SessionJournal::create(dir.path(), dir.path()).unwrap();
        journal.materialize().unwrap();
        let file = journal.path().to_path_buf();
        let header = journal.header().clone();
        // The real writer fails its next append without touching other files.
        std::fs::remove_file(&file).unwrap();
        std::fs::create_dir(&file).unwrap();
        let upstream = ara_testkit::FakeUpstream::start(
            ara_testkit::Script {
                responses: Vec::new(),
            },
            None,
        )
        .await
        .unwrap();
        let (mut host, mut rx) = fixture();
        host.session = Session::new(Some(journal), header, &[], None, None).unwrap();
        let origin = host.session.clone();
        let (before_entries, before_leaf) = {
            let journal = origin.journal.lock().await;
            (
                journal.entries().len(),
                journal.leaf_id().map(str::to_owned),
            )
        };
        host.config.model.base_url = upstream.base_url();
        host.config.max_model_calls = Some(1);
        host.start(
            Some(skill_input(UserSkillPrompt::new(
                UserContent::Text("receipt".into()),
                None,
            ))),
            Command::new(wire(json!({"type":"prompt","id":"failed-receipt"}))),
        )
        .await
        .unwrap();
        let mut active = host.active.take().unwrap();
        let completion = active.sink.clone();
        let result = tokio::time::timeout(Duration::from_secs(5), &mut active.task)
            .await
            .unwrap();
        assert_eq!(
            result
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap()
                .end,
            RunEnd::Aborted
        );
        host.completed(active, result).await;
        assert!(origin.persistence_error.lock().unwrap().is_some());
        assert!(host.connection.is_cancelled());
        assert!(
            upstream.requests.lock().await.is_empty(),
            "failed custom receipt must precede model/tool effects"
        );
        assert!(
            origin.messages.lock().unwrap().is_empty(),
            "failed MessageEnd cannot publish a Session message"
        );
        assert!(
            completion.messages.lock().unwrap().is_empty(),
            "failed MessageEnd cannot publish a completion receipt"
        );
        assert!(
            completion.entries.lock().unwrap().is_empty(),
            "failed MessageEnd cannot publish an entry receipt"
        );
        {
            let journal = origin.journal.lock().await;
            assert_eq!(
                journal.entries().len(),
                before_entries,
                "failed append cannot advance journal entries"
            );
            assert_eq!(
                journal.leaf_id().map(str::to_owned),
                before_leaf,
                "failed append cannot advance journal leaf"
            );
        }
        let emitted = frames(&mut rx);
        assert_eq!(
            emitted
                .iter()
                .filter(|frame| frame["type"] == "message_start"
                    && frame["message"]["role"] == "custom")
                .count(),
            1,
            "the admitted custom input starts exactly once"
        );
        assert_eq!(
            emitted
                .iter()
                .filter(
                    |frame| frame["type"] == "message_end" && frame["message"]["role"] == "custom"
                )
                .count(),
            0,
            "a failed journal receipt cannot publish MessageEnd"
        );
        assert!(
            emitted
                .iter()
                .all(|frame| frame["type"] != "tool_execution_start")
        );
        let failure = emitted
            .iter()
            .find(|frame| frame["type"] == "response")
            .unwrap();
        assert_eq!(failure["id"], "failed-receipt");
        assert!(
            failure["error"]
                .as_str()
                .unwrap()
                .contains("Session persistence failed")
        );
    }

    async fn observe_actual_rpc_terminal_and_journal() {
        native_loop_tests::observe_actual_loop_hooks().await;
        use ara_rpc::value_heap::{
            CapabilityId, NoRuntime, PropertyDescriptor, RuntimeReply, RuntimeRequest, ValueError,
            ValueHandle, ValueHeap, ValueRuntime,
        };
        let (host, mut rx) = fixture();
        let heap = ValueHeap::new().unwrap();
        let values = Arc::new(SessionValues::new(heap.clone(), Arc::new(NoRuntime)).unwrap());
        let header = json!({"type":"session","version":3,"id":"native-rpc-feedback"});
        let journal = SessionJournal::in_memory(header.clone()).unwrap();
        let session = Session::new(Some(journal), header, &[], Some(values.clone()), None).unwrap();
        {
            let mut journal = session.journal.lock().await;
            // Test-only profile for a shallow fixture; production profile admission remains open.
            session
                .projector
                .bind_native_session(
                    &mut journal,
                    Arc::new(
                        ara_cli::actual_session::GraphJournalEncoder::new(
                            values.clone(),
                            Some(128),
                        )
                        .unwrap(),
                    ),
                )
                .unwrap();
        }
        let root = heap
            .import_serde(
                &json!({"role":"assistant","content":[],"stopReason":"error",
            "errorMessage":"usage_limit_reached","providerPayload":{"private":"kept"}}),
            )
            .unwrap();
        let original = AgentMessage::from_graph(root.clone(), None).unwrap();
        let cancel = CancellationToken::new();
        let sink = Arc::new(RunSink {
            session: session.clone(),
            observed_usage: None,
            output: host.output.clone(),
            cancel: cancel.clone(),
            connection: host.connection.clone(),
            terminal: Mutex::new(None),
            messages: Mutex::new(Vec::new()),
            entries: Mutex::new(Vec::new()),
            header_guard: Mutex::new(GeminiHeaderGuard::new(false, 0)),
        });
        sink.emit(AgentEvent::MessageEnd {
            message: original.clone(),
        })
        .await;
        assert!(session.persistence_error.lock().unwrap().is_none());
        assert!(!cancel.is_cancelled());
        assert!(
            sink.entries.lock().unwrap().is_empty(),
            "no reconstructed typed receipt"
        );
        let entry_id = session.projector.entry_id(&original).unwrap();
        let journal = session.journal.lock().await;
        let entry = journal
            .entries()
            .iter()
            .find(|entry| entry.id == entry_id)
            .unwrap();
        assert!(
            entry
                .native_message
                .as_ref()
                .unwrap()
                .live_body()
                .unwrap()
                .same_identity(&root)
        );
        drop(journal);
        let public = frames(&mut rx);
        assert_eq!(public[0]["type"], "message_end");
        assert_eq!(public[0]["message"]["errorMessage"], "usage_limit_reached");
        assert!(public[0]["message"].get("providerPayload").is_none());
        assert!(matches!(
            heap.snapshot(
                &heap
                    .get(&root, &"providerPayload".into(), &NoRuntime)
                    .unwrap()
            )
            .unwrap(),
            ara_rpc::value_heap::ValueSnapshot::Object
        ));
        let report = RunReport {
            messages: vec![original.clone()],
            end: RunEnd::Error,
            stop_cause: None,
        };
        let (legacy, actual) = completed_assistant(&report, &session.projector).unwrap();
        assert!(legacy.is_none());
        assert!(actual.unwrap().same_record(&original));
        assert_eq!(
            actual_terminal_error(&original, &session.projector)
                .unwrap()
                .as_deref(),
            Some("usage_limit_reached")
        );
        let foreign = ValueHeap::new()
            .unwrap()
            .import_serde(&json!({"role":"assistant","content":[],"stopReason":"error"}))
            .unwrap();
        let foreign_report = RunReport {
            messages: vec![AgentMessage::from_graph(foreign, None).unwrap()],
            end: RunEnd::Error,
            stop_cause: None,
        };
        assert!(
            completed_assistant(&foreign_report, &session.projector)
                .unwrap_err()
                .downcast_ref::<ValueError>()
                .is_some()
        );

        struct Throws(ValueHandle);
        impl ValueRuntime for Throws {
            fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
                match request {
                    RuntimeRequest::Call {
                        capability: CapabilityId(19001),
                        ..
                    } => Err(ValueError::Thrown(self.0.clone())),
                    request => Err(ValueError::MissingCapability(request.capability())),
                }
            }
        }
        let throw_heap = ValueHeap::new().unwrap();
        let exception = throw_heap.object().unwrap();
        let throw_values = Arc::new(
            SessionValues::new(throw_heap.clone(), Arc::new(Throws(exception.clone()))).unwrap(),
        );
        let throw_projector = ara_cli::agent_message_projection::AgentMessageProjector::default();
        let mut throw_journal =
            SessionJournal::in_memory(json!({"type":"session","version":3,"id":"throw-role"}))
                .unwrap();
        throw_projector
            .bind_native_session(
                &mut throw_journal,
                Arc::new(
                    ara_cli::actual_session::GraphJournalEncoder::new(throw_values, None).unwrap(),
                ),
            )
            .unwrap();
        let throws = throw_heap
            .import_serde(&json!({"role":"assistant","content":[],"stopReason":"error"}))
            .unwrap();
        // Admit the legal own Text controls first. The live source may change;
        // the completion selector must retain a later original getter Throw.
        let throw_original = AgentMessage::from_graph(throws.clone(), None).unwrap();
        throw_heap
            .define_own(
                &throws,
                "role".into(),
                PropertyDescriptor::Accessor {
                    get: Some(ara_rpc::value_heap::CallableTarget::Host(CapabilityId(
                        19001,
                    ))),
                    set: None,
                    enumerable: true,
                    configurable: true,
                },
                &NoRuntime,
            )
            .unwrap();
        let throw_report = RunReport {
            messages: vec![throw_original],
            end: RunEnd::Error,
            stop_cause: None,
        };
        let error = completed_assistant(&throw_report, &throw_projector).unwrap_err();
        assert!(
            matches!(error.downcast_ref::<ValueError>(),Some(ValueError::Thrown(original))
            if original.same_identity(&exception)),
            "original getter exception is retained"
        );
    }

    async fn observe_edit_late_serial_host() {
        use ara_tools::edit_host::{EditDeferredDiagnostics, EditHostPort};
        let (mut host, mut rx) = fixture();
        let directory = tempfile::tempdir().unwrap();
        let session = host.session.clone();
        let owner = ara_cli::host_edit_session::HostEditSession::new(
            directory.path().to_path_buf(),
            Default::default(),
        );
        let mut tool_owner = super::super::CliToolOwner::default();
        tool_owner.edit_session = Some(owner.clone());
        tool_owner.context =
            Some(ara_tools::ToolContext::new(directory.path()).with_edit_host(owner.clone()));
        tool_owner.owned_builtins = super::super::owned_builtin_tools(
            tool_owner.context.as_ref().unwrap(),
            &["edit", "write"],
        );
        session.bind_edit_session(&tool_owner).await.unwrap();
        host.sessions.tool_owner = tool_owner.clone();
        *session.tool_owner.lock().unwrap() = tool_owner;
        let diagnostics = owner.deferred_diagnostics_host().unwrap();
        let deferred = EditDeferredDiagnostics::new(diagnostics.clone(), false);
        let path = directory.path().join("serial.rs");
        let raw = json!({"summary":"1 warning(s)","errored":false,"messages":["serial.rs:1:2 [warning] actual owner"]});
        let queued = deferred.begin(&path);
        queued.finalize(None);
        queued.on_deferred_diagnostics(raw.clone()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), owner.wake().notified())
            .await
            .unwrap();
        let hooks = RpcHooks {
            base: host.config.hooks.clone(),
            snapshot: host.snapshot.clone(),
            forced_child_yield: false,
            admission: None,
            publication: host.snapshot_publication.clone(),
            projector: session.projector.clone(),
            session: session.clone(),
        };
        let lazy = hooks.aside_messages().await.unwrap();
        assert_eq!(lazy.len(), 1);
        // The shared local WRITE success invalidates a queued EDIT fetch without
        // adding the deferred/LSP services that the full WRITE parent still lacks.
        ara_tools::write::WriteTool::new(host.sessions.tool_owner.context.clone().unwrap())
            .execute_os_values(path.to_str().unwrap(), "changed", CancellationToken::new())
            .await
            .unwrap();
        match &lazy[0] {
            ara_agent::AsideMessage::Lazy(factory) => assert!(factory().unwrap().is_none()),
            _ => panic!("expected source lazy aside"),
        }
        let queued = deferred.begin(&path);
        queued.finalize(None);
        queued.on_deferred_diagnostics(raw).unwrap();
        // EOF closes automatic admission at the real Host gate, including the
        // top-of-loop reconcile path that precedes the EOF break.
        host.input_closed = true;
        host.drain_queues = true;
        host.reconcile_queues().await;
        assert!(owner.has_pending() && host.active.is_none());
        host.input_closed = false;

        let cancel = host.connection.child_token();
        let sink = Arc::new(RunSink {
            session: session.clone(),
            observed_usage: None,
            output: host.output.clone(),
            cancel: cancel.clone(),
            connection: host.connection.clone(),
            terminal: Mutex::new(None),
            messages: Mutex::new(Vec::new()),
            entries: Mutex::new(Vec::new()),
            header_guard: Mutex::new(GeminiHeaderGuard::new(false, 0)),
        });
        let (release, released) = oneshot::channel();
        host.active = Some(ActiveRun {
            child_turn: None,
            native: None,
            cancel,
            task: tokio::spawn(async move {
                released.await.unwrap();
                Ok(None)
            }),
            command: Command::new(wire(json!({"type":"prompt","id":"settling"}))),
            sink,
        });
        host.drain_queues = true;
        host.reconcile_late_diagnostics().await;
        assert!(owner.has_pending(), "a settling Run keeps late work queued");
        release.send(()).unwrap();
        let active = host.active.take().unwrap();
        let mut active = active;
        let result = (&mut active.task).await;
        Box::pin(host.completed(active, result)).await;
        assert!(host.active.is_none());
        host.drain_queues = true;
        Box::pin(host.reconcile_late_diagnostics()).await;
        assert!(
            host.active.is_some(),
            "same Session idle wake starts ordinary prompt admission"
        );
        assert!(Arc::ptr_eq(&host.session, &session));
        let mut active = host.active.take().unwrap();
        let result = (&mut active.task).await;
        Box::pin(host.completed(active, result)).await;
        assert!(session.persistence_error.lock().unwrap().is_none());
        let journal = session.journal.lock().await;
        assert_eq!(
            journal
                .entries()
                .iter()
                .filter(|entry| entry
                    .agent_message_body()
                    .unwrap()
                    .is_some_and(|body| body.checked_legacy().unwrap()["customType"]
                        == "lsp_late_diagnostics"))
                .count(),
            1
        );
        drop(journal);
        let emitted = frames(&mut rx);
        assert!(emitted.iter().any(|frame| frame["type"] == "message_end"
            && frame["message"]["customType"] == "lsp_late_diagnostics"));
        assert!(
            !emitted
                .iter()
                .any(|frame| frame["type"] == "tool_execution_start")
        );
        // max_model_calls=0 in this controlled original Host fixture. The
        // observation proves scheduling/persistence, not a successful model Run.
    }

    #[tokio::test]
    async fn terminal_is_published_only_after_owned_run_settles() {
        observe_edit_late_serial_host().await;
        observe_actual_rpc_terminal_and_journal().await;
        let (mut host, mut rx) = fixture();
        let connection = host.connection.clone();
        let session = host.session.clone();
        let output = host.output.clone();
        let config = host.config.clone();
        let cancel = connection.child_token();
        let sink = Arc::new(RunSink {
            session,
            observed_usage: None,
            output,
            cancel: cancel.clone(),
            connection,
            terminal: Mutex::new(None),
            messages: Mutex::new(Vec::new()),
            entries: Mutex::new(Vec::new()),
            header_guard: Mutex::new(GeminiHeaderGuard::new(false, 0)),
        });
        let (reached_tx, reached) = oneshot::channel();
        let (release, release_rx) = oneshot::channel();
        let gate = Arc::new(TerminalGate {
            inner: sink.clone(),
            reached: Mutex::new(Some(reached_tx)),
            release: tokio::sync::Mutex::new(Some(release_rx)),
        });
        let agent = host.agent.clone();
        let run_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            agent
                .prompt_raw_with_config(
                    vec![skill_input(UserSkillPrompt::new(
                        UserContent::Text("first".into()),
                        Some(json!({"originalText":"/skill:proof"})),
                    ))],
                    config,
                    run_cancel,
                    gate,
                )
                .await
                .map(Some)
        });
        host.active = Some(ActiveRun {
            child_turn: None,
            native: None,
            cancel,
            task,
            command: Command::new(wire(json!({"type":"prompt","id":"first"}))),
            sink,
        });

        tokio::time::timeout(Duration::from_secs(5), reached)
            .await
            .unwrap()
            .unwrap();
        assert!(!host.active.as_ref().unwrap().task.is_finished());
        assert_eq!(host.state().await["isStreaming"], true);
        let live = frames(&mut rx);
        assert!(live.iter().any(|frame| frame["type"] == "agent_start"));
        let input = live
            .iter()
            .find(|frame| frame["type"] == "message_end")
            .unwrap()["message"]
            .clone();
        assert_eq!(input["customType"], "skill-prompt");
        assert!(!live.iter().any(|frame| frame["type"] == "agent_end"));

        release.send(()).unwrap();
        let mut active = host.active.take().unwrap();
        let result = tokio::time::timeout(Duration::from_secs(5), &mut active.task)
            .await
            .unwrap();
        host.completed(active, result).await;
        assert_eq!(host.state().await["isStreaming"], false);
        let settled = frames(&mut rx);
        assert_eq!(
            settled
                .iter()
                .filter(|frame| frame["type"] == "agent_end")
                .count(),
            1
        );
        assert_eq!(settled[0]["type"], "agent_end");
        assert_eq!(settled[0]["messages"][0], input);
        assert_eq!(settled[1]["id"], "first");
        assert_eq!(settled[1]["error"], "Model call limit reached");

        host.handle(Command::new(wire(
            json!({"type":"prompt","id":"next","message":"next"}),
        )))
        .await;
        let mut active = host.active.take().expect("immediate next prompt starts");
        let result = tokio::time::timeout(Duration::from_secs(5), &mut active.task)
            .await
            .unwrap();
        host.completed(active, result).await;
        let next = frames(&mut rx);
        assert_eq!(next[0]["id"], "next");
        assert_eq!(next[0]["success"], true);
        assert!(next.iter().any(|frame| frame["type"] == "agent_start"));
        assert_eq!(
            next.iter()
                .filter(|frame| frame["type"] == "agent_end")
                .count(),
            1
        );
        assert!(!next.iter().any(|frame| {
            frame["error"]
                .as_str()
                .is_some_and(|error| error.contains("already running"))
        }));
    }
    #[tokio::test]
    async fn public_summary_dto_getter_and_page_match_fixed_provider_view() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("provider-view.jsonl");
        let stamp = "2026-07-09T00:00:00.000Z";
        let header =
            json!({"type":"session","version":3,"id":"provider-view","cwd":"/","timestamp":stamp});
        let user = |id, parent, text| {
            json!({"type":"message","id":id,"parentId":parent,
            "timestamp":stamp,"message":Message::User(UserMessage::text(text))})
        };
        let entries = [
            user("m1", Value::Null, "before compaction"),
            json!({"type":"compaction","id":"c1","parentId":"m1","timestamp":stamp,
                "summary":"summary","firstKeptEntryId":"m1","tokensBefore":123,
                "shortSummary":"brief","tokensAfter":45,"warning":"ACTIVE warning","method":"snapcompact",
                "preserveData":{"snapcompact":{"frames":[{"data":"base64-frame","mimeType":"image/png",
                    "cols":10,"rows":10,"chars":100}],"totalChars":100,"truncatedChars":0,"textHead":"head","textTail":"tail"}}}),
            user("m2", json!("c1"), "after compaction"),
        ];
        let mut body = format!("{header}\n");
        for entry in entries {
            body.push_str(&format!("{entry}\n"));
        }
        std::fs::write(&path, &body).unwrap();
        let journal = SessionJournal::open(&path).unwrap();
        let public = Session::public_messages(&journal).unwrap();
        assert_eq!(
            public
                .iter()
                .map(|message| message["role"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["compactionSummary", "user", "user"]
        );
        let summary = &public[0];
        assert_eq!(summary["shortSummary"], "brief");
        assert_eq!(summary["tokensAfter"], 45);
        assert_eq!(summary["warning"], "ACTIVE warning");
        assert_eq!(summary["images"][0]["data"], "base64-frame");
        assert_eq!(
            summary["blocks"]
                .as_array()
                .unwrap()
                .iter()
                .map(|block| block["type"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["text", "image", "text"]
        );
        assert!(summary.get("preserveData").is_none());
        assert!(summary.get("frames").is_none());
        let (mut host, mut rx) = fixture();
        host.session = Session::new(Some(journal), header, &[], None, None).unwrap();
        host.handle(Command::new(wire(
            json!({"type":"get_messages","id":"all"}),
        )))
        .await;
        let all = frames(&mut rx);
        assert_eq!(all[0]["data"]["messages"], json!(public));
        host.handle(Command::new(wire(
            json!({"type":"get_messages_page","id":"first","limit":1}),
        )))
        .await;
        let first = frames(&mut rx);
        assert_eq!(first[0]["data"]["messages"], json!([summary]));
        assert_eq!(first[0]["data"]["totalMessages"], 3);
        let cursor = first[0]["data"]["nextCursor"].clone();
        assert!(cursor.is_string());
        host.handle(Command::new(wire(
            json!({"type":"get_messages_page","id":"tail","limit":2,"cursor":cursor}),
        )))
        .await;
        let tail = frames(&mut rx);
        assert_eq!(tail[0]["data"]["messages"], json!(&public[1..]));
        assert!(tail[0]["data"].get("nextCursor").is_none());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            body,
            "public views and paging never write history"
        );

        let exact = body.replace("\"textHead\":\"head\"", "\"textHead\":\"head \\ud800\"");
        std::fs::write(&path, exact).unwrap();
        let journal = SessionJournal::open(&path).unwrap();
        assert!(
            Session::public_messages(&journal).is_err(),
            "public boundary cannot replace malformed UTF16 archive text"
        );
    }
}
