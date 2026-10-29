//! Host-owned original tool membership and exact capability snapshots.
//! This composes the current Rust Builtin/MCP/RPC sources; it does not claim
//! complete fixed OMP factory, extension, or hidden-tool policy parity.

use crate::{
    actual_tools::{BuiltinToolSource, async_effect::NativeBuiltinRun, local_source::NativeLocalSource},
    mcp_native::NativeMcpSource,
    native_rpc_host::NativeRpcHostSource,
};
use ara_agent::{ActualProviderError, ActualToolBinding, ActualToolDecision, ActualToolSource, SessionValues,};
use ara_rpc::{
    WireString,value_heap::{PropertyDescriptor, RuntimeReply, RuntimeRequest, ValueError, ValueHandle, ValueRuntime, ValueSnapshot,
    },};
use std::sync::{Arc, Mutex, Weak};

type Result<T> = std::result::Result<T, ActualProviderError>;

#[derive(Clone)]
pub enum NativeCatalogOwner {
    Builtin(Arc<BuiltinToolSource>),
    Mcp(Arc<NativeMcpSource>),
    Rpc(Arc<NativeRpcHostSource>),
    Local(Arc<NativeLocalSource>),
}
impl NativeCatalogOwner {
    pub(crate) fn invoke_device(
        &self,
        binding: ActualToolBinding,
        request: crate::actual_tools::xdev_native::NativeMethodRequest,
    ) -> futures::future::BoxFuture<'static, Result<ValueHandle>> {
        match self {
            Self::Builtin(source) => source.clone().invoke_device(binding, request),
            Self::Mcp(source) => source.clone().invoke_device(binding, request),
            Self::Rpc(source) => source.clone().invoke_device(binding, request),
            Self::Local(source) => source.clone().invoke_device(binding, request),
        }
    }
    fn source(&self) -> Arc<dyn ActualToolSource> {
        match self {
            Self::Builtin(source) => source.clone(),
            Self::Mcp(source) => source.clone(),
            Self::Rpc(source) => source.clone(),
            Self::Local(source) => source.clone(),
        }
    }
    fn matches(&self, sources: &NativeCatalogSources, local_sources:&[Arc<NativeLocalSource>]) -> bool {
        match self {
            Self::Builtin(source) => Arc::ptr_eq(source, &sources.builtin),
            Self::Mcp(source) => sources.mcp.as_ref().is_some_and(|original| Arc::ptr_eq(source, original)),
            Self::Rpc(source) => sources.rpc.as_ref().is_some_and(|original| Arc::ptr_eq(source, original)),
            Self::Local(source) => local_sources.iter().any(|original|Arc::ptr_eq(source,original)),
        }
    }
    fn permission(&self, binding: &ActualToolBinding, values: &SessionValues) -> Result<ActualToolDecision> {
        match self {
            Self::Builtin(source) => source.permission(binding, values),
            Self::Mcp(source) => source.permission(binding, values),
            Self::Rpc(source) => source.permission(binding, values),
            Self::Local(source) => source.permission(binding, values),
        }
    }
}

#[derive(Clone)]

pub struct NativeCatalogMember {
    pub binding: ActualToolBinding,
    pub owner: NativeCatalogOwner,
}
impl NativeCatalogMember {
    pub fn bind(owner: NativeCatalogOwner, selected: &ValueHandle, values: &SessionValues) -> Result<Self> {
        let binding = owner.source().bind_selected(selected, values)?;
        Ok(Self { binding, owner })
    }
}

pub struct NativeCatalogSources {
    pub builtin: Arc<BuiltinToolSource>,
    pub mcp: Option<Arc<NativeMcpSource>>,
    pub rpc: Option<Arc<NativeRpcHostSource>>,
}

/// An immutable Host membership receipt. Metadata remains in its original
/// Graph objects. Runs retain this owner while current membership is refreshed.
pub struct NativeCatalogSnapshot {
    values: Weak<SessionValues>,
    collection: ValueHandle,
    members: Vec<NativeCatalogMember>,
    sources: NativeCatalogSources,
    local_sources:Vec<Arc<NativeLocalSource>>,
    devices: Option<NativeDevicePresentation>,
}
/// Session-owned handler for fixed plain-text xd://propose. It is not an
/// ordinary mounted JSON tool, and returns inner plan approval details only.
#[async_trait::async_trait]
pub trait NativeProposalHost: Send + Sync {
    fn is_active(&self) -> bool;
    async fn propose(&self, title: &str, cancel: tokio_util::sync::CancellationToken)
        -> std::result::Result<ara_agent::ToolOutput, ara_agent::ToolError>;
}

#[derive(Clone)]
pub struct NativeDeviceOptions {
    pub enabled: bool,
    pub explicit_builtins: bool,
    pub approval_host: Option<Arc<dyn crate::actual_tools::NativeDeviceApprovalHost>>,
    pub decorator: Option<Arc<dyn crate::actual_tools::NativeDeviceDecorator>>,
    /// Selected reference Host; consent/persistence never live in Core.
    pub report_issue: Option<crate::report_tool_issue::ReportIssueHost>,
    pub proposal_host: Option<Arc<dyn NativeProposalHost>>,
    pub bash_patterns:ara_rpc::WireValue,
    pub docs_mode:String,
    pub inline_patterns:Vec<WireString>,
    /// Exact Native Bun.Glob constructor, when the required consumer is installed.
    pub inline_glob_constructor:Option<ValueHandle>,
    /// Same-realm imported type constructor for original callable Local schemas.
    pub ark_type:Option<ValueHandle>,
    /// Explicit Host xdev state; None keeps normal discoverable policy.
    pub mounted_local_names:Option<Vec<WireString>>,
}
impl Default for NativeDeviceOptions{fn default()->Self{Self{enabled:false,explicit_builtins:false,approval_host:None,decorator:None,report_issue:None,proposal_host:None,bash_patterns:ara_rpc::WireValue::Array(Vec::new()),docs_mode:"builtins".into(),inline_patterns:Vec::new(),inline_glob_constructor:None,ark_type:None,mounted_local_names:None}}}
struct NativeDevicePresentation {
    /// One exact canonical registration, using fixed Map last-name-wins order.
    tools:ValueHandle,
    mounted_set:ValueHandle,
    active_set:ValueHandle,
    builtin_set:ValueHandle,
    docs_mode:String,
    inline_patterns:Vec<WireString>,
    inline_glob_constructor:Option<ValueHandle>,
    ark_type:Option<ValueHandle>,
    mounted: Vec<WireString>,
    builtins: Vec<WireString>,
    approval_host: Option<Arc<dyn crate::actual_tools::NativeDeviceApprovalHost>>,
    decorator: Option<Arc<dyn crate::actual_tools::NativeDeviceDecorator>>,
    report_issue: Option<crate::report_tool_issue::ReportIssueHost>,
    proposal_host: Option<Arc<dyn NativeProposalHost>>,
}
impl NativeCatalogSnapshot {
    pub fn new(
        values: &Arc<SessionValues>,
        original_array: ValueHandle,
        registry: Vec<NativeCatalogMember>,
        sources: NativeCatalogSources,
    ) -> Result<Arc<Self>> {
        Self::new_with_devices(
            values,
            original_array,
            registry,
            sources,
            NativeDeviceOptions::default(),
        )
    }
    pub(crate) fn new_with_devices(
        values: &Arc<SessionValues>,
        original_array: ValueHandle,
        registry: Vec<NativeCatalogMember>,
        sources: NativeCatalogSources,
        options: NativeDeviceOptions,
    ) -> Result<Arc<Self>> {
        Self::new_with_local_sources(values,original_array,registry,sources,options,Vec::new())
    }
    /// Host supplies explicit original Local owners; legacy callers retain empty locals.
    pub(crate) fn new_with_local_sources(
        values:&Arc<SessionValues>, original_array:ValueHandle, registry:Vec<NativeCatalogMember>,
        sources:NativeCatalogSources,options:NativeDeviceOptions,local_sources:Vec<Arc<NativeLocalSource>>,
    )->Result<Arc<Self>> {
        values.check_handle(&original_array)?;
        if let Some(ark_type)=&options.ark_type{values.check_handle(ark_type)?;}
        // A Host-authored ordinary Array can be checked without executing any
        // opaque getter/iterator. Original member metadata is never read here.
        if !matches!(values.heap().snapshot(&original_array)?, ValueSnapshot::Array) {
            return Err(ActualProviderError::Protocol("Native catalog requires the original ordinary tool Array"));
        }
        sources.builtin.current_tools(values)?;
        if let Some(source) = &sources.mcp {
            source.current_tools(values)?;
        }
        if let Some(source) = &sources.rpc {
            source.current_tools(values)?;
        }
        for source in &local_sources{source.current_tools(values)?;}
        for (index, member) in registry.iter().enumerate() {
            values.check_handle(&member.binding.source)?;
            if !member.owner.matches(&sources,&local_sources) {
                return Err(ActualProviderError::Protocol("Native catalog member belongs to another source owner"));
            }
            let original = member.owner.source().bind_selected(&member.binding.source, values)?;
            if !original.source.same_identity(&member.binding.source)
                || !Arc::ptr_eq(&original.capability, &member.binding.capability)
            {
                return Err(ActualProviderError::Protocol("Native catalog source/capability association differs"));
            }
            if member.binding.capability.actual_invoker().is_none() {
                return Err(ActualProviderError::Unsupported("Native catalog member has no real actual invoker"));
            }
            if registry[..index].iter().any(|old| old.binding.source.same_identity(&member.binding.source)) {
                return Err(ActualProviderError::Protocol("Native catalog original member is registered twice"));
            }
        }
        let length = values.heap().array_length(&original_array, values.runtime())?;
        for index in 0..length {
            let slot =
                values.heap().own_descriptor(&original_array, &index.to_string().as_str().into(), values.runtime())?;
            let Some(PropertyDescriptor::Data { value: selected, .. }) = slot else {
                return Err(ActualProviderError::Protocol("Native catalog presentation has a hole or accessor"));
            };
            if !registry.iter().any(|member| member.binding.source.same_identity(&selected)) {
                return Err(ActualProviderError::Protocol("Advertised Native tool has no original executable owner"));
            }
        }
        let heap = values.heap();
        let runtime = values.runtime();
        // Source declarations are installed by the Native module before catalog
        // preparation. A legacy source remains on its legacy constructor path.
        sources.builtin.configure_bash_approval(values,&heap.import_strict(options.bash_patterns.clone())?)?;
        let mut collection = original_array;
        let devices = if options.enabled {
            let tools=heap.new_map(&heap.undefined()?,runtime)?;
            let set=heap.get(&tools,&"set".into(),runtime)?;
            let mut builtins = Vec::new();
            for member in &registry {
                let name = heap.get(&member.binding.source, &"name".into(), runtime)?;
                let ValueSnapshot::String(name) = heap.snapshot(&name)? else {
                    return Err(ActualProviderError::Protocol(
                        "Canonical device registration name must be a string",
                    ));
                };
                heap.call(&set,&tools,&[heap.string(name.clone())?,member.binding.source.clone()],runtime)?;
                let builtin=match &member.owner{NativeCatalogOwner::Builtin(_)=>true,NativeCatalogOwner::Local(source)=>source.is_builtin(&name),_=>false};
                if builtin && !builtins.contains(&name)
                {
                    builtins.push(name);
                }
            }
            let mut mounted = Vec::new();
            let mut active = Vec::new();
            let mut kept = Vec::new();
            for index in 0..length {
                let selected =
                    heap.get(&collection, &index.to_string().as_str().into(), runtime)?;
                let member = registry
                    .iter()
                    .find(|member| member.binding.source.same_identity(&selected))
                    .ok_or(ActualProviderError::Protocol(
                        "Device presentation lost canonical member",
                    ))?;
                let name = heap.get(&selected, &"name".into(), runtime)?;
                let ValueSnapshot::String(name) = heap.snapshot(&name)? else {
                    return Err(ActualProviderError::Protocol(
                        "Device presentation name is not a string",
                    ));
                };
                let mount=match (&member.owner,&options.mounted_local_names){
                    // Fixed original Local fixtures supply an explicit xdev mounted
                    // state; never fabricate loadMode on their source declaration.
                    (NativeCatalogOwner::Local(_),Some(names))=>names.contains(&name),
                    _=>(!matches!(member.owner,NativeCatalogOwner::Builtin(_))||!options.explicit_builtins)
                        &&crate::actual_tools::xdev_native::is_mountable(&selected,runtime)?,
                };
                if mount {
                    if !mounted.contains(&name) {
                        mounted.push(name);
                    }
                } else {
                    if !active.contains(&name) {
                        active.push(name);
                    }
                    kept.push(selected);
                }
            }
            collection = heap.array(kept)?;
            let mounted_set=heap.new_set(&heap.array(mounted.iter().map(|name|heap.string(name.clone())).collect::<std::result::Result<Vec<_>,_>>()?)?,runtime)?;
            let active_set=heap.new_set(&heap.array(active.iter().map(|name|heap.string(name.clone())).collect::<std::result::Result<Vec<_>,_>>()?)?,runtime)?;
            for member in &registry {
                let name=heap.get(&member.binding.source,&"name".into(),runtime)?;
                if matches!(heap.snapshot(&name)?,ValueSnapshot::String(value) if value.equals_ascii("write")) && matches!(member.owner,NativeCatalogOwner::Builtin(_)) {
                    let prior=heap.get(&member.binding.source,&"approval".into(),runtime)?;
                    let approval=crate::actual_tools::xdev_native::write_approval_function(heap,&tools,&mounted_set,&active_set,&prior)?;
                    heap.define_own_data(&member.binding.source,"approval".into(),approval,runtime)?;
                }
            }
            let builtin_set=heap.new_set(&heap.array(builtins.iter().map(|name|heap.string(name.clone())).collect::<std::result::Result<Vec<_>,_>>()?)?,runtime)?;
            Some(NativeDevicePresentation {
                tools,
                mounted_set,
                active_set,
                builtin_set,
                docs_mode:options.docs_mode,
                inline_patterns:options.inline_patterns,
                inline_glob_constructor:options.inline_glob_constructor,
                ark_type:options.ark_type,
                mounted,
                builtins,
                approval_host: options.approval_host,
                decorator: options.decorator,
                report_issue: options.report_issue,
                proposal_host: options.proposal_host,
            })
        } else {
            None
        };
        let snapshot = Arc::new(Self {
            values: Arc::downgrade(values),
            collection,
            members: registry,
            sources,
            local_sources,
            devices,
        });
        snapshot
            .sources
            .builtin
            .bind_device_snapshot(&snapshot, values)?;
        if let Some(source)=&snapshot.sources.mcp{source.bind_device_snapshot(&snapshot,values)?;}
        if let Some(source)=&snapshot.sources.rpc{source.bind_device_snapshot(&snapshot,values)?;}

        Ok(snapshot)
    }
    pub(crate) fn check_values(&self, values: &SessionValues) -> Result<()> {
        let original = self.values.upgrade().ok_or(ActualProviderError::Protocol("Native catalog Session expired"))?;
        if !std::ptr::eq(original.as_ref(), values) {
            return Err(ValueError::ForeignHeap.into());
        }
        Ok(())
    }
    fn member(&self, selected: &ValueHandle) -> Option<&NativeCatalogMember> {
        self.members.iter().find(|member| member.binding.source.same_identity(selected))
    }
    fn exact_member(&self, binding: &ActualToolBinding) -> Option<&NativeCatalogMember> {
        self.member(&binding.source).filter(|member| Arc::ptr_eq(&member.binding.capability, &binding.capability))
    }
    /// Exact registry membership for direct and fallback wrapper execution;
    /// active top-level tools are intentionally not restricted to mounted-only.
    pub(crate) fn wrapper_member(&self,receiver:&ValueHandle,values:&SessionValues)->Result<NativeCatalogMember>{
        self.check_values(values)?;
        let member=self.member(receiver).ok_or(ActualProviderError::Protocol("Root wrapper has no exact canonical source"))?;
        let original=member.owner.source().bind_selected(receiver,values)?;
        if !Arc::ptr_eq(&original.capability,&member.binding.capability){return Err(ActualProviderError::Protocol("Root wrapper canonical capability differs"))}
        match member.owner.permission(&member.binding,values)? {
            ActualToolDecision::Allow=>Ok(member.clone()),
            ActualToolDecision::Block(_)=>Err(ValueError::TypeError("Original root source grant was revoked".into()).into()),
        }
    }
    pub(crate) fn device_member_in(
        &self,
        name: &WireString,
        mounted_only: bool,
        values: &SessionValues,
        runtime: &dyn ValueRuntime,
    ) -> Result<Option<NativeCatalogMember>> {
        self.check_values(values)?;
        let Some(devices) = &self.devices else {
            return Ok(None);
        };
        let heap=values.heap();
        let selected=crate::actual_tools::xdev_native::resolve_canonical_device_source(
            &devices.tools,&devices.mounted_set,&devices.active_set,mounted_only,name,runtime)?;
        if matches!(heap.kind(&selected)?,ara_rpc::value_heap::ValueKind::Undefined) {return Ok(None)}
        let member=self.member(&selected).ok_or(ActualProviderError::Protocol("Canonical device Map returned an unowned source"))?;
        Ok(Some(member.clone()))
    }
    /// Names actually mounted in this immutable Host catalog snapshot.
    pub fn device_names(&self) -> &[WireString] {
        self.devices
            .as_ref()
            .map(|devices| devices.mounted.as_slice())
            .unwrap_or_default()
    }
    /// Whether this snapshot owns an allocated device namespace.
    pub fn devices_enabled(&self) -> bool {
        self.devices.is_some()
    }
    pub(crate) fn report_issue_host(&self)->Option<&crate::report_tool_issue::ReportIssueHost>{
        self.devices.as_ref().and_then(|devices|devices.report_issue.as_ref())
    }
    pub(crate) fn proposal_host(&self) -> Option<&Arc<dyn NativeProposalHost>> {
        self.devices.as_ref().and_then(|devices| devices.proposal_host.as_ref())
    }
    pub(crate) fn device_approval(
        &self,
    ) -> Result<Arc<dyn crate::actual_tools::NativeDeviceApprovalHost>> {
        self.devices
            .as_ref()
            .and_then(|devices| devices.approval_host.clone())
            .ok_or(ActualProviderError::Unsupported(
                "Native device Host approval adapter is absent",
            ))
    }
    pub(crate) fn device_ark_type(&self)->Option<ValueHandle>{self.devices.as_ref().and_then(|devices|devices.ark_type.clone())}
    pub(crate) fn device_decorator(
        &self,
    ) -> Option<Arc<dyn crate::actual_tools::NativeDeviceDecorator>> {
        self.devices
            .as_ref()
            .and_then(|devices| devices.decorator.clone())
    }
    pub(crate) fn device_listing(
        &self,
        values: &SessionValues,
        runtime: &dyn ValueRuntime,
    ) -> Result<WireString> {
        self.check_values(values)?;
        let Some(devices) = &self.devices else {
            return Err(ActualProviderError::Unsupported(
                "Native device transport is absent",
            ));
        };
        let mut rows =
            vec![format!("xd:// {} mounted tool devices.", devices.mounted.len()).into()];
        for name in &devices.mounted {
            let Some(member) = self.device_member_in(name, true, values, runtime)? else {
                continue;
            };
            let (name,summary,_dynamic) = crate::actual_tools::xdev_native::device_summary_entry(
                &member.binding.source,&devices.builtin_set,runtime)?;
            let padding = " ".repeat(14usize.saturating_sub(name.units().len()));
            rows.push(crate::actual_tools::xdev_native::concat(&[
                "xd://".into(),
                name.clone(),
                padding.into(),
                " ".into(),
                summary,
            ]));
        }
        rows.push("".into());
        rows.push("Read xd://<tool> for docs + JSON schema; write the JSON args object to xd://<tool> to execute. Active top-level tools accept the same dispatch.".into());
        Ok(crate::actual_tools::xdev_native::join(&rows, "\n"))
    }
    /// Copy original Map/Set presentation containers without registering any
    /// executable identity. Contained sources remain the production originals.
    pub fn docs_metadata(&self,values:&SessionValues,runtime:&dyn ValueRuntime)->Result<Option<crate::actual_tools::xdev_docs::NativeXdevDocsMetadata>>{
        self.check_values(values)?;
        let Some(devices)=&self.devices else{return Ok(None)};
        let heap=values.heap();
        Ok(Some(crate::actual_tools::xdev_docs::NativeXdevDocsMetadata{
            tools:heap.new_map(&devices.tools,runtime)?,
            mounted_names:heap.new_set(&devices.mounted_set,runtime)?,
            active_names:heap.new_set(&devices.active_set,runtime)?,
            builtin_names:heap.new_set(&devices.builtin_set,runtime)?,
            glob_constructor:devices.inline_glob_constructor.clone(),
        }))
    }
    /// Production docsAll delegates to the same original metadata consumer.
    pub fn device_prompt_docs(&self,values:&SessionValues,runtime:&dyn ValueRuntime)->Result<WireString>{
        self.check_values(values)?;
        let Some(devices)=&self.devices else{return Ok("".into())};
        let raw_mode=ara_rpc::WireValue::String(devices.docs_mode.clone().into());
        let raw_patterns=ara_rpc::WireValue::Array(devices.inline_patterns.iter().cloned().map(ara_rpc::WireValue::String).collect());
        self.docs_metadata(values,runtime)?.expect("checked devices").prompt_docs(&raw_mode,&raw_patterns,runtime)
    }
    pub fn advertised_capabilities(&self,values:&SessionValues)->Result<Vec<Arc<dyn ara_agent::AgentTool>>>{
        self.check_values(values)?;
        let heap=values.heap();let mut tools=Vec::new();
        for index in 0..heap.array_length(&self.collection,values.runtime())?{
            let selected=heap.get(&self.collection,&index.to_string().as_str().into(),values.runtime())?;
            tools.push(self.member(&selected).ok_or(ActualProviderError::Protocol("Native presentation lost original capability"))?.binding.capability.clone());
        }
        Ok(tools)
    }
    pub fn builtin(&self) -> &Arc<BuiltinToolSource> {
        &self.sources.builtin
    }
    pub fn mcp(&self) -> Option<&Arc<NativeMcpSource>> {
        self.sources.mcp.as_ref()
    }
    pub fn rpc(&self) -> Option<&Arc<NativeRpcHostSource>> {
        self.sources.rpc.as_ref()
    }
}

struct CatalogState {
    current: Arc<NativeCatalogSnapshot>,
    retired: Vec<Weak<NativeCatalogSnapshot>>,
    active: Option<(Weak<NativeCatalogSnapshot>, Weak<NativeBuiltinRun>)>,
    context_sources: Vec<Weak<BuiltinToolSource>>,
}
pub struct NativeToolCatalog {
    values: Weak<SessionValues>,
    self_ref:Weak<Self>,
    state: Mutex<CatalogState>,
}
impl NativeToolCatalog {
    pub fn new(values: &Arc<SessionValues>, snapshot: Arc<NativeCatalogSnapshot>) -> Result<Arc<Self>> {
        snapshot.check_values(values)?;
        let context_sources = vec![Arc::downgrade(snapshot.builtin())];
        let catalog=Arc::new_cyclic(|original|Self {
            values: Arc::downgrade(values),self_ref:original.clone(),
            state: Mutex::new(CatalogState { current: snapshot.clone(), retired: Vec::new(), active: None, context_sources }),
        });
        for source in &snapshot.local_sources{source.bind_catalog(&catalog.self_ref,values)?;}
        Ok(catalog)
    }
    pub fn check_values(&self, values: &SessionValues) -> Result<()> {
        let original = self.values.upgrade().ok_or(ActualProviderError::Protocol("Native catalog Session expired"))?;
        if !std::ptr::eq(original.as_ref(), values) {
            return Err(ValueError::ForeignHeap.into());
        }
        Ok(())
    }
    pub fn snapshot(&self, values: &SessionValues) -> Result<Arc<NativeCatalogSnapshot>> {
        self.check_values(values)?;
        Ok(self.state.lock().unwrap_or_else(|e| e.into_inner()).current.clone())
    }
    fn snapshots(&self) -> Vec<Arc<NativeCatalogSnapshot>> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.retired.retain(|snapshot| snapshot.strong_count() != 0);
        let mut snapshots = vec![state.current.clone()];
        snapshots.extend(state.retired.iter().filter_map(Weak::upgrade));
        snapshots
    }
    /// Host publishes after its actual refresh setup and removal-grant
    /// lifecycle succeed. An active lease registers new ports before publication.
    /// Old leases keep their exact snapshot;
    /// this method never cancels, replays, or transfers entered effects.
    pub(crate) fn publish(
        &self,
        snapshot: Arc<NativeCatalogSnapshot>,
        values: &SessionValues,
        active: Option<&Arc<NativeBuiltinRun>>,
    ) -> Result<()> {
        self.check_values(values)?;
        snapshot.check_values(values)?;
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(run)=active{run.check_values(values)?;run.check_live()?;}
        if let Some((_, old)) = &state.active {
            if old.upgrade().is_some_and(|run| run.check_live().is_ok()) {
                let Some(run) = active else {
                    return Err(ActualProviderError::Protocol(
                        "Active Native catalog refresh requires its original Run lease",
                    ));
                };
                if !old.upgrade().is_some_and(|old| Arc::ptr_eq(&old, run)) {
                    return Err(ActualProviderError::Protocol("Native catalog refresh belongs to another Run lease"));
                }
                run.check_values(values)?;
                run.check_live()?;
            }
        }
        // Binding performs no programmable Graph Get/Call. All preparation
        // checks precede publication and exact old-grant revocation.
        for source in &snapshot.local_sources{
            source.bind_catalog(&self.self_ref,values)?;
            if let Some(run)=active{source.bind_run(run,values)?;}
        }
        if Arc::ptr_eq(&state.current,&snapshot){return Ok(())}
        for member in &state.current.members{
            if snapshot.exact_member(&member.binding).is_none(){
                if let NativeCatalogOwner::Local(source)=&member.owner{source.revoke_binding(&member.binding);}
            }
        }
        state.retired.retain(|old| old.strong_count() != 0);
        state.context_sources.retain(|old| old.strong_count() != 0);
        if !state
            .context_sources
            .iter()
            .filter_map(Weak::upgrade)
            .any(|source| Arc::ptr_eq(&source, snapshot.builtin()))
        {
            state.context_sources.push(Arc::downgrade(snapshot.builtin()));
        }
        let old = Arc::downgrade(&state.current);
        state.retired.push(old);
        state.current = snapshot.clone();
        if let Some(run) = active {
            state.active = Some((Arc::downgrade(&snapshot), Arc::downgrade(run)));
        }
        Ok(())
    }
    pub(crate) fn bind_run_snapshot(
        &self,
        snapshot: &Arc<NativeCatalogSnapshot>,
        run: &Arc<NativeBuiltinRun>,
        values: &SessionValues,
    ) -> Result<()> {
        self.check_values(values)?;
        snapshot.check_values(values)?;
        run.check_values(values)?;
        run.check_live()?;
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !Arc::ptr_eq(&state.current, snapshot)
            && !state.retired.iter().filter_map(Weak::upgrade).any(|old| Arc::ptr_eq(&old, snapshot))
        {
            return Err(ActualProviderError::Protocol("Native Run snapshot was never published by this catalog"));
        }
        let same_run=if let Some(old)=state.active.as_ref().and_then(|(_,old)|old.upgrade()){
            let same=Arc::ptr_eq(&old,run);
            if !same&&old.check_live().is_ok(){return Err(ActualProviderError::Protocol("Native catalog already has an active Run"))}
            same
        }else{false};
        // Even same-Run refresh snapshots need their original Local locators.
        for source in &snapshot.local_sources{source.bind_catalog(&self.self_ref,values)?;source.bind_run(run,values)?;}
        if same_run{return Ok(())}
        state.active = Some((Arc::downgrade(snapshot), Arc::downgrade(run)));
        Ok(())
    }
    pub(crate) fn execution_snapshot(&self, values: &SessionValues) -> Result<Arc<NativeCatalogSnapshot>> {
        self.check_values(values)?;
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((snapshot, run)) = &state.active {
            if run.upgrade().is_some_and(|run| run.check_live().is_ok()) {
                return snapshot.upgrade().ok_or(ActualProviderError::Protocol("Native active snapshot expired"));
            }
        }
        Ok(state.current.clone())
    }
    pub fn permission(&self, binding: &ActualToolBinding, values: &SessionValues) -> Result<ActualToolDecision> {
        self.check_values(values)?;
        values.check_handle(&binding.source)?;
        for snapshot in self.snapshots() {
            if let Some(member) = snapshot.exact_member(binding) {
                return member.owner.permission(binding, values);
            }
        }
        Err(ActualProviderError::Protocol("Native catalog permission source/capability differs"))
    }
    pub(crate) fn resolve_context_signal(
        &self,
        request: &RuntimeRequest<'_>,
    ) -> Option<std::result::Result<RuntimeReply, ValueError>> {
        let sources: Vec<_> = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.context_sources.retain(|source| source.strong_count() != 0);
            state.context_sources.iter().filter_map(Weak::upgrade).collect()
        };
        for source in sources {
            if let Some(reply) = source.resolve_context_signal(request) {
                return Some(reply);
            }
        }
        None
    }
}
impl ActualToolSource for NativeToolCatalog {
    fn current_tools(&self, values: &SessionValues) -> Result<Option<ValueHandle>> {
        Ok(Some(self.snapshot(values)?.collection.clone()))
    }
    fn bind_selected(&self, selected: &ValueHandle, values: &SessionValues) -> Result<ActualToolBinding> {
        self.check_values(values)?;
        values.check_handle(selected)?;
        for snapshot in self.snapshots() {
            if let Some(member) = snapshot.member(selected) {
                return Ok(member.binding.clone());
            }
        }
        Err(ActualProviderError::Protocol("Native selected original tool has no retained exact owner"))
    }
    fn has_fallback(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .current
            .devices
            .is_some()
    }
    fn resolve_fallback(
        &self,
        name: &ValueHandle,
        values: &SessionValues,
    ) -> Result<Option<ActualToolBinding>> {
        self.check_values(values)?;
        values.check_handle(name)?;
        let ValueSnapshot::String(name) = values.heap().snapshot(name)? else {
            return Ok(None);
        };
        let name = crate::actual_tools::xdev_native::strip_prefix(&name);
        Ok(self
            .execution_snapshot(values)?
            .device_member_in(&name, true, values, values.runtime())?
            .map(|member| member.binding))
    }
}
