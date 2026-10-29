//! Assemble Native sources from the Host's completed authoritative tool setup.
//! Discovery, filtering, transports, selection order and grants stay in the
//! caller. A refresh prepares an unpublished snapshot in its retained realm.

use super::{
    NativeCatalogFactoryImports, PreparedNativeSession,
    catalog::{
        NativeCatalogMember, NativeCatalogOwner, NativeCatalogSnapshot, NativeCatalogSources, NativeToolCatalog,
    },
};
use crate::{
    actual_tools::{BuiltinKind, BuiltinToolSource, bash_native::SessionBashOwner},
    mcp_native::{NativeMcpSource, local::LocalRoutes},
    native_logger::logger_host::NativeLoggerHostOptions,
    native_realm::NativeRealm,
    native_rpc_host::{NativeRpcHostOwner, NativeRpcHostSource},
};
use ara_agent::{ActualProviderError, ActualToolSource, AgentTool};
use ara_mcp::{McpTool, native_graph::NativeMcpImports};
use ara_rpc::value_heap::{PropertyDescriptor, ValueHandle};
use ara_tools::ToolContext;
use std::sync::Arc;

type Result<T> = std::result::Result<T, ActualProviderError>;

/// Provenance selected by authoritative setup, never inferred from tool names.
/// MCP/RPC indices refer to the supplied exact owner vectors, including owners
/// that are registered but are not currently advertised.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeHostToolSelection {
    Builtin(BuiltinKind),
    Mcp(usize),
    Rpc(usize),
}

pub struct NativeHostCatalogInputs {
    pub context: ToolContext,
    pub builtin_kinds: Vec<BuiltinKind>,
    pub builtin_grants: Vec<bool>,
    /// Exact connected owners returned by the existing async connect_owned.
    pub mcp_tools: Vec<Arc<McpTool>>,
    pub mcp_grants: Vec<bool>,
    pub mcp_routes: LocalRoutes,
    /// Original connection/pending owners returned by ToolBridge::native_owners.
    pub rpc_owners: Vec<Arc<dyn NativeRpcHostOwner>>,
    pub rpc_grants: Vec<bool>,
    /// One entry for every advertised tool, in the original setup order.
    pub advertised: Vec<NativeHostToolSelection>,
    /// Fresh logical Sessions supply a new owner; refresh retains the same Arc.
    pub bash_session: Arc<SessionBashOwner>,
}

/// The capabilities below are the exact bindings in snapshot. The Host updates
/// its provenance with them; the original MCP transport Arcs remain unchanged.
/// This value does not publish membership or attach sources to an active Run.
pub struct NativeHostCatalogPreparation {
    pub snapshot: Arc<NativeCatalogSnapshot>,
    pub tools: Vec<Arc<dyn AgentTool>>,
    pub builtin_tools: Vec<Arc<dyn AgentTool>>,
    pub mcp_tools: Vec<Arc<dyn AgentTool>>,
    pub rpc_tools: Vec<Arc<dyn AgentTool>>,
}

pub struct NativeHostSessionPreparation {
    pub session: Arc<PreparedNativeSession>,
    pub catalog: NativeHostCatalogPreparation,
}

fn selection_index(
    selected: NativeHostToolSelection,
    builtins: &[BuiltinKind],
    mcp_count: usize,
    rpc_count: usize,
) -> Result<usize> {
    match selected {
        NativeHostToolSelection::Builtin(kind) => builtins
            .iter()
            .position(|original| *original == kind)
            .ok_or(ActualProviderError::Protocol("Advertised builtin has no authoritative setup owner")),
        NativeHostToolSelection::Mcp(index) if index < mcp_count => Ok(builtins.len() + index),
        NativeHostToolSelection::Rpc(index) if index < rpc_count => Ok(builtins.len() + mcp_count + index),
        NativeHostToolSelection::Mcp(_) => {
            Err(ActualProviderError::Protocol("Advertised MCP tool has no authoritative setup owner"))
        }
        NativeHostToolSelection::Rpc(_) => {
            Err(ActualProviderError::Protocol("Advertised RPC tool has no authoritative setup owner"))
        }
    }
}

fn register_source(
    realm: &NativeRealm,
    source: &dyn ActualToolSource,
    owner: NativeCatalogOwner,
    expected_count: usize,
    registry: &mut Vec<NativeCatalogMember>,
) -> Result<()> {
    let values = &realm.values;
    let collection = source
        .current_tools(values)?
        .ok_or(ActualProviderError::Protocol("Authoritative Native source has no original tool collection"))?;
    let heap = values.heap();
    if heap.array_length(&collection, values.runtime())? as usize != expected_count {
        return Err(ActualProviderError::Protocol("Native source differs from its authoritative setup owner count"));
    }
    for index in 0..expected_count {
        let slot = heap.own_descriptor(&collection, &index.to_string().as_str().into(), values.runtime())?;
        let Some(PropertyDescriptor::Data { value: selected, .. }) = slot else {
            return Err(ActualProviderError::Protocol("Authoritative Native source has a hole or accessor"));
        };
        registry.push(NativeCatalogMember::bind(owner.clone(), &selected, values)?);
    }
    Ok(())
}

/// Prepare sources and exact advertised bindings only. Fresh callers invoke
/// this after Logger preparation through finish_host_session_with_logger;
/// retained callers pass the same prepared Session realm and Bash owner.
pub fn prepare_host_catalog(
    realm: &Arc<NativeRealm>,
    inputs: NativeHostCatalogInputs,
) -> Result<NativeHostCatalogPreparation> {
    prepare_host_catalog_with_devices(realm,inputs,super::catalog::NativeDeviceOptions::default())
}
pub fn prepare_host_catalog_with_devices(
    realm:&Arc<NativeRealm>,inputs:NativeHostCatalogInputs,devices:super::catalog::NativeDeviceOptions,
)->Result<NativeHostCatalogPreparation>{
    prepare_host_catalog_with_child(realm,inputs,devices,None)
}
/// Add only the exact child-owned original yield source; ordinary callers keep
/// their existing sources, selection order and grants.
pub fn prepare_host_catalog_with_child(
    realm:&Arc<NativeRealm>,inputs:NativeHostCatalogInputs,devices:super::catalog::NativeDeviceOptions,
    child:Option<Arc<crate::child_contract::NativeYieldBundle>>,
)->Result<NativeHostCatalogPreparation>{
    if inputs.builtin_grants.len() != inputs.builtin_kinds.len()
        || inputs.mcp_grants.len() != inputs.mcp_tools.len()
        || inputs.rpc_grants.len() != inputs.rpc_owners.len()
    {
        return Err(ActualProviderError::Protocol("Native setup must supply one current grant per exact owner"));
    }
    let builtin_count = inputs.builtin_kinds.len();
    let mcp_count = inputs.mcp_tools.len();
    let rpc_count = inputs.rpc_owners.len();
    let mut selected_indices = Vec::with_capacity(inputs.advertised.len());
    for selected in &inputs.advertised {
        let index = selection_index(*selected, &inputs.builtin_kinds, mcp_count, rpc_count)?;
        if selected_indices.contains(&index) {
            return Err(ActualProviderError::Protocol("Native setup advertises the same original owner twice"));
        }
        selected_indices.push(index);
    }

    let values = &realm.values;
    let wait_session=inputs.bash_session.wait_session();
    let builtin =
        BuiltinToolSource::new_with_bash_session(values, inputs.context, &inputs.builtin_kinds, inputs.bash_session)?;
    if !Arc::ptr_eq(&wait_session,&builtin.wait_session()){return Err(ActualProviderError::Protocol("Native Wait Session owner changed during assembly"));}
    builtin.bind_ast_module(realm.ast.clone(), values)?;
    for (kind, granted) in inputs.builtin_kinds.iter().zip(&inputs.builtin_grants) {
        builtin.set_granted(*kind, *granted)?;
    }
    let mcp = if mcp_count == 0 {
        None
    } else {
        realm.globals.install_mcp_dependencies(values.runtime())?;
        let errors = realm.tool_error_bindings()?;
        let imports = NativeMcpImports {
            environment: realm.globals.environment.clone(),
            format_failure: errors.format_failure.clone(),
            errors,
        };
        let source =
            NativeMcpSource::new(values, inputs.mcp_tools, imports, inputs.mcp_routes, realm.globals.errors.clone())?;
        for (index, granted) in inputs.mcp_grants.into_iter().enumerate() {
            source.set_enabled(index, granted)?;
        }
        Some(source)
    };
    let rpc = if rpc_count == 0 {
        None
    } else {
        let source = NativeRpcHostSource::new(
            values,
            inputs.rpc_owners,
            realm.globals.environment.clone(),
            realm.globals.errors.clone(),
        )?;
        for (index, granted) in inputs.rpc_grants.into_iter().enumerate() {
            source.set_enabled(index, granted)?;
        }
        Some(source)
    };

    let mut registry = Vec::with_capacity(builtin_count + mcp_count + rpc_count);
    register_source(
        realm,
        builtin.as_ref(),
        NativeCatalogOwner::Builtin(builtin.clone()),
        builtin_count,
        &mut registry,
    )?;
    if let Some(source) = &mcp {
        register_source(realm, source.as_ref(), NativeCatalogOwner::Mcp(source.clone()), mcp_count, &mut registry)?;
    }
    if let Some(source) = &rpc {
        register_source(realm, source.as_ref(), NativeCatalogOwner::Rpc(source.clone()), rpc_count, &mut registry)?;
    }
    let capabilities = registry.iter().map(|member| member.binding.capability.clone()).collect::<Vec<_>>();
    let builtin_tools = capabilities[..builtin_count].to_vec();
    let mcp_tools = capabilities[builtin_count..builtin_count + mcp_count].to_vec();
    let rpc_tools = capabilities[builtin_count + mcp_count..].to_vec();
    let mut advertised =
        selected_indices.iter().map(|index| registry[*index].binding.source.clone()).collect::<Vec<ValueHandle>>();
    let mut locals=Vec::new();
    if let Some(child)=child.filter(|child|child.owner.config().structured){
        if !Arc::ptr_eq(child.owner.values(),values){
            return Err(ActualProviderError::Protocol("Child yield belongs to another catalog realm"));
        }
        for original in &advertised {
            let name=values.heap().get(original,&"name".into(),values.runtime())?;
            if matches!(values.heap().snapshot(&name)?,ara_rpc::value_heap::ValueSnapshot::String(s) if s.equals_ascii("yield")){
                return Err(ActualProviderError::Protocol("Another source cannot impersonate child-owned yield"));
            }
        }
        let originals=values.heap().array(vec![child.original.clone()])?;
        let source=crate::actual_tools::local_source::NativeLocalSource::new(
            values,originals,vec![child.definition()?],vec!["yield".into()])?;
        let member=NativeCatalogMember::bind(NativeCatalogOwner::Local(source.clone()),&child.original,values)?;
        child.bind_capability(member.binding.capability.clone());
        advertised.push(child.original.clone());registry.push(member);locals.push(source);
    }
    let snapshot = NativeCatalogSnapshot::new_with_local_sources(
        values,
        values.heap().array(advertised)?,
        registry,
        NativeCatalogSources { builtin, mcp, rpc },
        devices,locals,
    )?;
    let tools=snapshot.advertised_capabilities(values
    )?;
    Ok(NativeHostCatalogPreparation { snapshot, tools, builtin_tools, mcp_tools, rpc_tools })
}

/// Finish the same fresh draft after the Host's async setup. Logger installs
/// its original globals before MCP module imports are evaluated by the builder.
pub fn finish_host_session_with_logger(
    draft: &Arc<PreparedNativeSession>,
    inputs: NativeHostCatalogInputs,
    host_options: NativeLoggerHostOptions,
) -> Result<NativeHostSessionPreparation> {
    finish_host_session_with_logger_and_devices(draft,inputs,host_options,super::catalog::NativeDeviceOptions::default())
}
pub fn finish_host_session_with_logger_and_devices(
    draft:&Arc<PreparedNativeSession>,inputs:NativeHostCatalogInputs,host_options:NativeLoggerHostOptions,
    devices:super::catalog::NativeDeviceOptions,
)->Result<NativeHostSessionPreparation>{
    finish_host_session_with_logger_devices_and_child(draft,inputs,host_options,devices,None)
}
pub fn finish_host_session_with_logger_devices_and_child(
    draft:&Arc<PreparedNativeSession>,inputs:NativeHostCatalogInputs,host_options:NativeLoggerHostOptions,
    devices:super::catalog::NativeDeviceOptions,child:Option<Arc<crate::child_contract::NativeYieldBundle>>,
)->Result<NativeHostSessionPreparation>{
    let (session, catalog) = draft.finish_catalog_bindings_with_logger_builder(host_options, |realm| {
        let prepared = prepare_host_catalog_with_child(realm, inputs, devices,child)?;
        let catalog = NativeToolCatalog::new(&realm.values, prepared.snapshot.clone())?;
        Ok((
            NativeCatalogFactoryImports {
                catalog,
                string_function: realm.globals.string.clone(),
                bun: realm.globals.bun.clone(),
                original_logger: None,
            },
            prepared,
        ))
    })?;
    Ok(NativeHostSessionPreparation { session, catalog })
}
