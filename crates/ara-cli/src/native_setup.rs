//! Bin-only Native preparation from the completed authoritative CliSetup.
//! Host selection, connection, settings and publication remain in main/RPC.

use super::{CliHooks, CliSetup};
use anyhow::{Context as _, Result, bail};
use ara_agent::{
    ActualToolBinding, ActualToolDecision, ActualToolSource, AgentTool, LoopHooks, OwnedProviderContextPort,
    SessionValues,
};
use ara_ai::Model;
use ara_cli::{
    actual_tools::{
        BuiltinKind,
        bash_native::{BashSessionSettings, SessionBashOwner},
    },
    mcp_native::local::LocalRoutes,
    native_factory::{
        PreparedNativeSession,
        catalog::NativeCatalogSnapshot,
        host::{
            NativeHostCatalogInputs, NativeHostCatalogPreparation, NativeHostToolSelection,
        },
    },
    native_logger::logger_host::NativeLoggerHostOptions,
    native_rpc_host::NativeRpcHostOwner,
};
use ara_context::DateCwdReminder;
use ara_mcp::McpTool;
use ara_rpc::value_heap::PropertyDescriptor;
use std::{path::Path, sync::Arc};

/// Exactly one explicit current grant per authoritative input owner. For a
/// same-realm known owner, preparation reads its existing grant instead.
pub(super) struct NativeSetupGrants {
    pub builtin: Vec<bool>,
    pub mcp: Vec<bool>,
    pub rpc: Vec<bool>,
}

pub(super) struct NativeSetupHostOptions {
    /// Required for a fresh realm; retained refresh must not rebuild Logger.
    pub logger: Option<NativeLoggerHostOptions>,
    pub local_routes: LocalRoutes,
    pub bash_settings: BashSessionSettings,
    pub grants: NativeSetupGrants,
    /// Existing selected ports are reused. A different supplied Arc is rejected.
    /// A missing port is selected only from this explicit Host input.
    pub provider_port: Option<Arc<dyn OwnedProviderContextPort>>,
    pub proposal_host: Option<Arc<dyn ara_cli::native_factory::catalog::NativeProposalHost>>,
}

pub(super) struct NativeBuiltinLineage {
    pub kind: BuiltinKind,
    pub binding: ActualToolBinding,
}
pub(super) struct NativeMcpLineage {
    pub owner: Arc<McpTool>,
    pub binding: ActualToolBinding,
}
pub(super) struct NativeRpcLineage {
    pub owner: Arc<dyn NativeRpcHostOwner>,
    pub binding: ActualToolBinding,
}

/// Keep both original transport/connection owners and their exact graph
/// bindings. Advertisement names are not provenance or grant authority.
pub(super) struct NativeCliOwner {
    pub peer_ingress: Arc<super::peer_host::PeerIngress>,
    pub prepared: Arc<PreparedNativeSession>,
    pub snapshot: Arc<NativeCatalogSnapshot>,
    pub bash_session: Arc<SessionBashOwner>,
    pub wait_session:Arc<ara_cli::actual_tools::wait_native::SessionWaitOwner>,
    pub task_session:Arc<ara_cli::actual_tools::task_native::SessionTaskOwner>,
    pub service_session: Option<Arc<ara_cli::host_services::SessionServiceLease>>,
    pub child_contract: Option<Arc<ara_cli::child_contract::ChildContractOwner>>,
    pub child_yield: Option<Arc<ara_cli::child_contract::NativeYieldBundle>>,
    pub builtin: Vec<NativeBuiltinLineage>,
    pub mcp: Vec<NativeMcpLineage>,
    pub rpc: Vec<NativeRpcLineage>,
}

fn granted(decision: ActualToolDecision) -> bool {
    matches!(decision, ActualToolDecision::Allow)
}

impl NativeCliOwner {
    pub fn builtin_grant(&self, kind: BuiltinKind) -> Result<Option<bool>> {
        let Some(member) = self.builtin.iter().find(|member| member.kind == kind) else {
            return Ok(None);
        };
        self.snapshot
            .builtin()
            .permission(&member.binding, self.prepared.values())
            .map(granted)
            .map(Some)
            .map_err(Into::into)
    }

    pub fn mcp_grant(&self, owner: &Arc<McpTool>) -> Result<Option<bool>> {
        let Some(member) = self.mcp.iter().find(|member| Arc::ptr_eq(&member.owner, owner)) else {
            return Ok(None);
        };
        let source = self.snapshot.mcp().context("Native MCP lineage has no exact source")?;
        source.permission(&member.binding, self.prepared.values()).map(granted).map(Some).map_err(Into::into)
    }

    /// None means this exact owner is unknown, not that a new grant is allowed.
    /// A Host rebuilding connection adapters supplies an explicit current grant
    /// after its own authoritative connection/definition migration decision.
    pub fn rpc_grant(&self, owner: &Arc<dyn NativeRpcHostOwner>) -> Result<Option<bool>> {
        let Some(member) = self.rpc.iter().find(|member| Arc::ptr_eq(&member.owner, owner)) else {
            return Ok(None);
        };
        let source = self.snapshot.rpc().context("Native RPC lineage has no exact source")?;
        source.permission(&member.binding, self.prepared.values()).map(granted).map(Some).map_err(Into::into)
    }
}

#[derive(Clone)]
pub(super) struct NativeSetupPublication {
    pub prepared: Arc<PreparedNativeSession>,
    pub pending_snapshot: Arc<NativeCatalogSnapshot>,
    pub native_owner: Arc<NativeCliOwner>,
    pub hooks: Arc<dyn LoopHooks>,
    pub tools: Vec<Arc<dyn AgentTool>>,
}

/// Only authoritative owned_builtins may enter this fixed name-to-kind map.
pub(super) fn builtin_kind(tool: &Arc<dyn AgentTool>) -> Result<BuiltinKind> {
    Ok(match tool.definition().name.as_str() {
        "read" => BuiltinKind::Read,
        "write" => BuiltinKind::Write,
        "edit" => BuiltinKind::Edit,
        "bash" => BuiltinKind::Bash,
        "wait"=>BuiltinKind::Wait,
        "task"=>BuiltinKind::Task,
        "grep" => BuiltinKind::Grep,
        "glob" => BuiltinKind::Glob,
        "ast_grep" => BuiltinKind::AstGrep,
        "ast_edit" => BuiltinKind::AstEdit,
        "debug" => BuiltinKind::Debug,
        "web_search" => BuiltinKind::WebSearch,
        _ => bail!("Authoritative CLI builtin has no supported Native kind"),
    })
}

fn add_selection(matches: &mut Vec<NativeHostToolSelection>, selected: NativeHostToolSelection) {
    if !matches.contains(&selected) {
        matches.push(selected);
    }
}

fn advertised_selection(
    setup: &CliSetup,
    kinds: &[BuiltinKind],
    captured_rpc: &[(Arc<dyn AgentTool>, Arc<dyn NativeRpcHostOwner>)],
    prior: Option<&NativeCliOwner>,
) -> Result<Vec<NativeHostToolSelection>> {
    let mut advertised = Vec::with_capacity(setup.tools.len());
    for tool in &setup.tools {
        // The child builder appends its original local source. Preserve exact source identity,
        // never treat an unrelated same-name yield as the owned declaration.
        if prior.and_then(|owner|owner.child_yield.as_ref()).is_some_and(|bundle|bundle.is_capability(tool)) {continue;}
        let mut matches = Vec::new();
        for (index, owned) in setup.tool_owner.owned_builtins.iter().enumerate() {
            if Arc::ptr_eq(owned, tool) {
                add_selection(&mut matches, NativeHostToolSelection::Builtin(kinds[index]));
            }
        }
        for (index, owner) in setup.tool_owner.owned_mcp.iter().enumerate() {
            let original: Arc<dyn AgentTool> = owner.clone();
            if Arc::ptr_eq(&original, tool)
                || prior.is_some_and(|prior| {
                    prior.mcp.iter().any(|member| {
                        Arc::ptr_eq(&member.owner, owner) && Arc::ptr_eq(&member.binding.capability, tool)
                    })
                })
            {
                add_selection(&mut matches, NativeHostToolSelection::Mcp(index));
            }
        }
        for (index, (original, owner)) in captured_rpc.iter().enumerate() {
            if Arc::ptr_eq(original, tool)
                || prior.is_some_and(|prior| {
                    prior.rpc.iter().any(|member| {
                        Arc::ptr_eq(&member.owner, owner) && Arc::ptr_eq(&member.binding.capability, tool)
                    })
                })
            {
                add_selection(&mut matches, NativeHostToolSelection::Rpc(index));
            }
        }
        if matches.len() != 1 {
            bail!("Advertised CLI tool must have exactly one authoritative Arc owner");
        }
        advertised.push(matches[0]);
    }
    Ok(advertised)
}

fn exact_bindings(
    source: &dyn ActualToolSource,
    capabilities: &[Arc<dyn AgentTool>],
    values: &SessionValues,
) -> Result<Vec<ActualToolBinding>> {
    let collection = source.current_tools(values)?.context("Native source has no original tool collection")?;
    if values.heap().array_length(&collection, values.runtime())? as usize != capabilities.len() {
        bail!("Native exact binding count differs from prepared capabilities");
    }
    let mut bindings = Vec::with_capacity(capabilities.len());
    for (index, capability) in capabilities.iter().enumerate() {
        let slot = values.heap().own_descriptor(&collection, &index.to_string().as_str().into(), values.runtime())?;
        let Some(PropertyDescriptor::Data { value: selected, .. }) = slot else {
            bail!("Native exact binding collection has a hole or accessor");
        };
        let binding = source.bind_selected(&selected, values)?;
        if !Arc::ptr_eq(&binding.capability, capability) {
            bail!("Prepared Native capability differs from its exact source binding");
        }
        bindings.push(binding);
    }
    Ok(bindings)
}

fn validate_owners(
    setup: &CliSetup,
    kinds: &[BuiltinKind],
    captured_rpc: &[(Arc<dyn AgentTool>, Arc<dyn NativeRpcHostOwner>)],
) -> Result<()> {
    for (index, kind) in kinds.iter().enumerate() {
        if kinds[..index].contains(kind) {
            bail!("Authoritative CLI builtin kind is registered twice");
        }
    }
    for (index, owner) in setup.tool_owner.owned_mcp.iter().enumerate() {
        if setup.tool_owner.owned_mcp[..index].iter().any(|old| Arc::ptr_eq(old, owner)) {
            bail!("Exact MCP transport owner is registered twice");
        }
    }
    for (index, (tool, owner)) in captured_rpc.iter().enumerate() {
        if captured_rpc[..index]
            .iter()
            .any(|(old_tool, old_owner)| Arc::ptr_eq(old_tool, tool) || Arc::ptr_eq(old_owner, owner))
        {
            bail!("Exact RPC adapter or connection owner is registered twice");
        }
    }
    Ok(())
}

/// Prepare only an unpublished CLI setup. The caller checks all other Session
/// consumers, adopts the pending snapshot with the active Run lease if needed,
/// and publishes the complete candidate through its existing transaction.
pub(super) fn prepare_cli_native(
    setup: &mut CliSetup,
    draft: Arc<PreparedNativeSession>,
    captured_rpc: Vec<(Arc<dyn AgentTool>, Arc<dyn NativeRpcHostOwner>)>,
    cwd: &Path,
    model: &Model,
    host: NativeSetupHostOptions,
 ) -> Result<NativeSetupPublication> {
    prepare_cli_native_owned(setup,draft,captured_rpc,cwd,model,host,None)
}

pub(super) fn prepare_cli_native_child(
    setup:&mut CliSetup,draft:Arc<PreparedNativeSession>,captured_rpc:Vec<(Arc<dyn AgentTool>,Arc<dyn NativeRpcHostOwner>)>,
    cwd:&Path,model:&Model,host:NativeSetupHostOptions,child:Arc<ara_cli::child_contract::ChildContractOwner>,
)->Result<NativeSetupPublication>{prepare_cli_native_owned(setup,draft,captured_rpc,cwd,model,host,Some(child))}

fn prepare_cli_native_owned(
    setup:&mut CliSetup,draft:Arc<PreparedNativeSession>,captured_rpc:Vec<(Arc<dyn AgentTool>,Arc<dyn NativeRpcHostOwner>)>,
    cwd:&Path,model:&Model,host:NativeSetupHostOptions,requested_child:Option<Arc<ara_cli::child_contract::ChildContractOwner>>,
) -> Result<NativeSetupPublication> {
    let context = setup.tool_owner.context.clone().context("Native CLI setup needs its authoritative ToolContext")?;
    if context.cwd.as_path() != cwd {
        bail!("Native CLI ToolContext and selected cwd differ");
    }
    let xdev_settings=super::rpc_host_settings::xdev_settings(cwd,&super::ara_home().join("agent"))?;
    let devices=ara_cli::native_factory::catalog::NativeDeviceOptions{
        enabled:xdev_settings.enabled&&setup.tool_owner.owned_builtins.iter().any(|tool|tool.definition().name=="write"),
        explicit_builtins:setup.explicit_tools,
        approval_host:Some(Arc::new(ara_cli::actual_tools::NoUiDeviceApproval{
            mode:xdev_settings.approval_mode.into(),policies:xdev_settings.approval_policies,
        })),
        decorator:None,
        report_issue:Some(super::cli_report_issue_host(cwd,model)?),
        proposal_host:host.proposal_host.clone(),
        ark_type:None,
        mounted_local_names:None,
        bash_patterns:xdev_settings.bash_patterns,
        docs_mode:xdev_settings.docs_mode,
        inline_patterns:xdev_settings.inline_patterns.into_iter().map(Into::into).collect(),
        inline_glob_constructor:{
            let globals=&draft.realm().globals;
            Some(ara_cli::actual_tools::install_bun_glob(draft.values().heap(),&globals.bun,&globals.errors.error,draft.values().runtime())?)
        },
    };
    let prior = setup.tool_owner.native.clone();
    let same_realm = prior.as_ref().is_some_and(|prior| Arc::ptr_eq(prior.prepared.values(), draft.values()));
    if same_realm && !prior.as_ref().is_some_and(|prior| Arc::ptr_eq(&prior.prepared, &draft)) {
        bail!("Same-realm refresh must retain the original prepared Native Session");
    }
    let child_contract=requested_child.or_else(||prior.as_ref().filter(|_|same_realm).and_then(|owner|owner.child_contract.clone()));
    let child_yield=match child_contract.as_ref() {
        Some(owner)=>Some(match prior.as_ref().filter(|_|same_realm).and_then(|old|old.child_yield.clone()) {
            Some(bundle)=>bundle,
            None=>ara_cli::child_contract::NativeYieldBundle::prepare(owner.clone(),draft.realm())?,
        }),
        None=>None,
    };
    let kinds = setup.tool_owner.owned_builtins.iter().map(builtin_kind).collect::<Result<Vec<_>>>()?;
    validate_owners(setup, &kinds, &captured_rpc)?;
    let advertised = advertised_selection(setup, &kinds, &captured_rpc, prior.as_deref())?;
    let mut grants = host.grants;
    if grants.builtin.len() != kinds.len()
        || grants.mcp.len() != setup.tool_owner.owned_mcp.len()
        || grants.rpc.len() != captured_rpc.len()
    {
        bail!("Host must explicitly supply one current grant per authoritative owner");
    }
    if same_realm {
        let prior = prior.as_ref().expect("same_realm requires prior lineage");
        for (kind, grant) in kinds.iter().zip(&mut grants.builtin) {
            if let Some(current) = prior.builtin_grant(*kind)? {
                *grant = current;
            }
        }
        for (owner, grant) in setup.tool_owner.owned_mcp.iter().zip(&mut grants.mcp) {
            if let Some(current) = prior.mcp_grant(owner)? {
                *grant = current;
            }
        }
        for ((_, owner), grant) in captured_rpc.iter().zip(&mut grants.rpc) {
            if let Some(current) = prior.rpc_grant(owner)? {
                *grant = current;
            }
        }
    }
    let bash_session = if same_realm {
        prior.as_ref().expect("same_realm requires prior lineage").bash_session.clone()
    } else {
        SessionBashOwner::new(host.bash_settings)
    };

    // Validate the Host's provider selection before assembling any fresh source.
    let selected = setup.context_controller.binding();
    if selected.actual_values.as_ref().is_some_and(|values| !Arc::ptr_eq(values, draft.values())) {
        bail!("CLI controller has selected different actual Session values");
    }
    let port = match (selected.owned_port.as_ref(), host.provider_port) {
        (Some(current), Some(proposed)) if !Arc::ptr_eq(current, &proposed) => {
            bail!("CLI controller has selected a different actual provider port");
        }
        (Some(current), _) => current.clone(),
        (None, Some(proposed)) => proposed,
        (None, None) => bail!("Host must select the missing actual provider port"),
    };
    let inputs = NativeHostCatalogInputs {
        context,
        builtin_kinds: kinds.clone(),
        builtin_grants: grants.builtin,
        mcp_tools: setup.tool_owner.owned_mcp.clone(),
        mcp_grants: grants.mcp,
        mcp_routes: host.local_routes,
        rpc_owners: captured_rpc.iter().map(|(_, owner)| owner.clone()).collect(),
        rpc_grants: grants.rpc,
        advertised,
        bash_session: bash_session.clone(),
    };
    let (prepared, catalog): (Arc<PreparedNativeSession>, NativeHostCatalogPreparation) = if same_realm {
        if host.logger.is_some() {
            bail!("Retained Native refresh cannot replace its original Logger");
        }
        draft.require_logger()?;
        draft.require_catalog()?;
        let catalog = ara_cli::native_factory::host::prepare_host_catalog_with_child(draft.realm(), inputs, devices,child_yield.clone())?;
        (draft, catalog)
    } else {
        let logger = host.logger.context("Fresh Native setup needs Host Logger options")?;
        let finished = ara_cli::native_factory::host::finish_host_session_with_logger_devices_and_child(&draft, inputs, logger, devices,child_yield.clone())?;
        (finished.session, finished.catalog)
    };
    if let Some(previous)=prior.as_ref().filter(|_|same_realm){
        catalog.snapshot.builtin().retain_task_session(previous.task_session.clone())?;
    }
    let task_session=catalog.snapshot.builtin().task_session();
    let device_prompt=catalog.snapshot.device_prompt_docs(prepared.values(),prepared.values().runtime())?;
    if let Some(previous)=setup.xdev_prompt.take(){if let Some(index)=setup.system_prompt.iter().rposition(|entry|entry==&previous){setup.system_prompt.remove(index);}}
    if !device_prompt.is_empty(){
        let prompt=device_prompt.to_utf8().context("native system prompt requires valid UTF-8")?;
        setup.system_prompt.push(prompt.clone());setup.xdev_prompt=Some(prompt);
    }
    let native_hooks = prepared.require_tool_hooks()?.clone();
    native_hooks.check_values(prepared.values())?;
    let builtin_bindings =
        exact_bindings(catalog.snapshot.builtin().as_ref(), &catalog.builtin_tools, prepared.values())?;
    let mcp_bindings = if let Some(source) = catalog.snapshot.mcp() {
        exact_bindings(source.as_ref(), &catalog.mcp_tools, prepared.values())?
    } else {
        if !catalog.mcp_tools.is_empty() {
            bail!("Prepared MCP capabilities have no exact Native source");
        }
        Vec::new()
    };
    let rpc_bindings = if let Some(source) = catalog.snapshot.rpc() {
        exact_bindings(source.as_ref(), &catalog.rpc_tools, prepared.values())?
    } else {
        if !catalog.rpc_tools.is_empty() {
            bail!("Prepared RPC capabilities have no exact Native source");
        }
        Vec::new()
    };
    let service_session = if setup.bind_native_tool_context {
        match prior.as_ref().filter(|_| same_realm).and_then(|owner| owner.service_session.clone()) {
            Some(lease) => Some(lease),
            None => Some(ara_cli::host_services::SessionServiceLease::new(
                cwd,
                &super::ara_home().join("run").join("services"),
                std::env::current_exe()?,
                // This point exposes the typed Host producer, not a model tool.
                // Future WRITE admission supplies its actual current launch policy.
                ara_cli::host_services::ServicePolicy { launch_enabled: false },
            )?),
        }
    } else { None };
    let peer_ingress = prior.as_ref().filter(|_| same_realm).map(|owner| owner.peer_ingress.clone()).unwrap_or_else(|| Arc::new(super::peer_host::PeerIngress::default()));
    let wait_session=bash_session.wait_session();
    wait_session.bind_service(service_session.clone());
    let native_owner = Arc::new(NativeCliOwner {
        peer_ingress: peer_ingress.clone(),
        prepared: prepared.clone(),
        snapshot: catalog.snapshot.clone(),
        bash_session,
        wait_session,
        task_session,
        service_session: service_session.clone(),
        child_contract:child_contract.clone(),
        child_yield:child_yield.clone(),
        builtin: kinds
            .into_iter()
            .zip(builtin_bindings)
            .map(|(kind, binding)| NativeBuiltinLineage { kind, binding })
            .collect(),
        mcp: setup
            .tool_owner
            .owned_mcp
            .iter()
            .cloned()
            .zip(mcp_bindings)
            .map(|(owner, binding)| NativeMcpLineage { owner, binding })
            .collect(),
        rpc: captured_rpc
            .into_iter()
            .zip(rpc_bindings)
            .map(|((_, owner), binding)| NativeRpcLineage { owner, binding })
            .collect(),
    });
    let hooks: Arc<dyn LoopHooks> = Arc::new(CliHooks {
        peer_ingress: Some(peer_ingress),
        edit_session: setup.tool_owner.edit_session.clone(),
        edit_native_session: Some(prepared.clone()),
        service_session,
        child_contract,
        child_yield,
        reminder: DateCwdReminder::new(),
        pending_resolution: setup
            .tool_owner
            .context
            .as_ref()
            .map(|ctx| ctx.pending_resolution.clone())
            .unwrap_or_default(),
        actual_reminder: ara_cli::actual_host::GraphDateCwdReminder::default(),
        native_tool_hooks: Some(native_hooks),
        bind_native_tool_context: setup.bind_native_tool_context,
        cwd: cwd.to_string_lossy().replace('\\', "/"),
        context_controller: setup.context_controller.clone(),
        model: model.clone(),
        projector: setup.projector.clone(),
        projection_journal: setup.projection_journal.clone(),
    });
    // Never replace an existing selection. Incomplete unpublished candidates
    // stage the exact Host-selected port and values only after all preparation.
    if selected.actual_values.is_none() || selected.owned_port.is_none() {
        setup.context_controller.stage_actual_binding(prepared.values().clone(), port)?;
    }
    let publication = NativeSetupPublication {
        prepared,
        pending_snapshot: catalog.snapshot,
        native_owner: native_owner.clone(),
        hooks: hooks.clone(),
        tools: catalog.tools.clone(),
    };
    setup.tools = catalog.tools;
    setup.hooks = hooks;
    setup.tool_owner.owned_builtins = catalog.builtin_tools;
    setup.tool_owner.native = Some(native_owner);
    Ok(publication)
}

/// Process/connection-owned cleanup. Session jobs do not join a foreground Run.
pub(super) type NativeSessionJobs = Arc<std::sync::Mutex<Vec<Arc<SessionBashOwner>>>>;
pub(super) fn remember_session_jobs(jobs: &NativeSessionJobs, owner: &Arc<NativeCliOwner>) {
    let mut jobs = jobs.lock().unwrap_or_else(|e| e.into_inner());
    if !jobs.iter().any(|job| Arc::ptr_eq(job, &owner.bash_session)) {
        jobs.push(owner.bash_session.clone());
    }
}
pub(super) async fn close_session_jobs(jobs: &NativeSessionJobs) -> Result<()> {
    let owners = std::mem::take(&mut *jobs.lock().unwrap_or_else(|e| e.into_inner()));
    for owner in &owners {
        owner.close();
    }
    let settled = futures::future::join_all(owners.iter().map(|owner| owner.close_and_settle())).await;
    let unknown = settled
        .into_iter()
        .flatten()
        .filter(|receipt| receipt.uncertain())
        .map(|receipt| receipt.summary())
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        bail!("Session Bash jobs have unknown outcomes: {}", unknown.join("; "));
    }
    Ok(())
}

pub(super) async fn close_bash_owner(owner: &Arc<SessionBashOwner>) -> Result<()> {
    let receipts = owner.close_and_settle().await;
    let unknown =
        receipts.into_iter().filter(|receipt| receipt.uncertain()).map(|receipt| receipt.summary()).collect::<Vec<_>>();
    if !unknown.is_empty() {
        bail!("Session Bash jobs have unknown outcomes: {}", unknown.join("; "));
    }
    Ok(())
}

