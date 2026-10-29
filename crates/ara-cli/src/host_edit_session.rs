//! Session-owned EDIT facade and passive late diagnostics, fixed OMP 596f2da.
//! Sources: sdk.ts:263-292,1853,3925; session/yield-queue.ts; local-protocol.ts.
//! Copyright (c) 2025-2026 Can Bölük; (c) 2026 Stencil Labs, Inc.
//! MIT; see THIRD_PARTY_NOTICES.md. Supplied ACP/LSP services stay optional.

use crate::{
    agent_message_projection::AgentMessageProjector, mcp_native::local::LocalRoutes,
    native_factory::PreparedNativeSession, session_artifacts::SessionArtifacts,
};
use ara_agent::{AgentMessage, AsideCallbacks, AsideError, AsideMessage, ToolError};
use ara_rpc::value_heap::{
    CompiledFunctionForm, CompiledFunctionSpec, NoRuntime, ValueError, ValueHandle, ValueHeap, ValueRuntime,
};
use ara_session::SessionJournal;
use ara_tools::edit_host::{
    EditClientBridge, EditDeferredDiagnosticEntry, EditDeferredDiagnosticsHost, EditDiagnosticsSession,
    EditHostPathPolicy, EditHostPort, EditLspOptions, EditLspWritethrough,
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
};
use tokio::sync::Notify;

const LATE_TYPE: &str = "lsp_late_diagnostics";
const LATE_TEMPLATE: &str = include_str!("host_edit_late_diagnostic.md");

/// Services are supplied by the Host before building its tool snapshot. None
/// is authoritative absence; this facade does not manufacture an ACP/LSP client.
#[derive(Clone, Default)]
pub struct EditSuppliedServices {
    pub enable_lsp: Option<bool>,
    pub client_bridge: Option<Arc<dyn EditClientBridge>>,
    pub lsp: Option<Arc<dyn EditLspWritethrough>>,
    pub file_fallbacks: Option<Arc<ara_tools::edit_file_fallback::EditFileMutationFallbackRegistry>>,
    pub vault: Option<Arc<crate::host_vault::HostVault>>,
    pub lsp_options: EditLspOptions,
    pub write_lsp_options: Option<EditLspOptions>,
}

/// Host-owned current planning policy. Core tools only consume this narrow view.
pub trait EditPlanPolicy: Send + Sync {
    fn current_plan_path(&self) -> Option<String>;
    /// Current reference survives leaving plan mode; None is not a default plan.
    fn read_reference_path(&self) -> Option<PathBuf> { None }
    fn read_reference_snapshot(&self) -> Result<ara_tools::read::ReadPlanReferenceSnapshot, ToolError> {
        Ok(self.read_reference_path().map_or(ara_tools::read::ReadPlanReferenceSnapshot::Absent, ara_tools::read::ReadPlanReferenceSnapshot::Path))
    }
}

#[derive(Default)]
struct ProtocolServices {
    bridge: Option<Arc<dyn EditClientBridge>>,
    plan: Option<Arc<dyn EditPlanPolicy>>,
}

#[derive(Clone)]
struct Identity {
    id: String,
    artifacts: Arc<SessionArtifacts>,
    local_root: PathBuf,
}

#[derive(Default)]
struct NotifyingDiagnostics {
    state: EditDiagnosticsSession,
    wake: Arc<Notify>,
}
impl EditDeferredDiagnosticsHost for NotifyingDiagnostics {
    fn bump_file_mutation_version(&self, path: &Path) -> u64 {
        self.state.bump_file_mutation_version(path)
    }
    fn get_file_mutation_version(&self, path: &Path) -> u64 {
        self.state.get_file_mutation_version(path)
    }
    fn reduce_diagnostics(&self, path: &Path, result: Value) -> std::io::Result<Value> {
        self.state.reduce_diagnostics(path, result)
    }
    fn queue_deferred_diagnostics(&self, entry: EditDeferredDiagnosticEntry) -> std::io::Result<()> {
        self.state.queue_deferred_diagnostics(entry)?;
        // One coalesced permit; the serial Host checks the actual queue and its
        // active/settlement gates. This never steers or interrupts a tool.
        self.wake.notify_one();
        Ok(())
    }
}

/// Created with ToolContext, retained by CLI hooks/Session, installed before
/// EditTool captures services. A new Session gets a new owner even though OMP's
/// EditStore is retained. Binding supplies the actual journal/artifact identity.
pub struct HostEditSession {
    home: PathBuf,
    identity: RwLock<Option<Identity>>,
    local_cells: Mutex<Vec<ValueHandle>>,
    diagnostics: Arc<NotifyingDiagnostics>,
    supplied: EditSuppliedServices,
    protocol: Arc<RwLock<ProtocolServices>>,
}
impl HostEditSession {
    pub fn new(home: PathBuf, supplied: EditSuppliedServices) -> Arc<Self> {
        Arc::new(Self {
            home,
            identity: RwLock::new(None),
            local_cells: Mutex::new(Vec::new()),
            diagnostics: Arc::new(NotifyingDiagnostics::default()),
            supplied,
            protocol: Arc::new(RwLock::new(ProtocolServices::default())),
        })
    }
    pub fn bind(&self, journal: &SessionJournal, artifacts: Arc<SessionArtifacts>) -> anyhow::Result<()> {
        let local_root = crate::mcp_native::local::session_root(artifacts.directory(), journal.session_id())?;
        let identity = Identity { id: journal.session_id().to_owned(), artifacts, local_root };
        let mut current = self.identity.write().unwrap_or_else(|e| e.into_inner());
        if current.as_ref().is_some_and(|current| current.id != identity.id) {
            anyhow::bail!("EDIT facade cannot replace its originating Session identity");
        }
        for cell in self.local_cells.lock().unwrap_or_else(|e| e.into_inner()).iter() {
            set_local_cell(cell, &identity)?;
        }
        *current = Some(identity);
        Ok(())
    }
    pub fn fresh(&self) -> Arc<Self> {
        Self::new(self.home.clone(), self.supplied.clone())
    }
    pub fn fresh_with_supplied(&self, supplied: EditSuppliedServices) -> Arc<Self> {
        Self::new(self.home.clone(), supplied)
    }
    /// Retained Session rebind updates immutable process/settings supply while
    /// preserving its identity and shared ledger; old admitted Arcs remain valid.
    pub fn with_supplied(&self, supplied: EditSuppliedServices) -> Arc<Self> {
        Arc::new(Self {
            home: self.home.clone(),
            identity: RwLock::new(self.identity.read().unwrap_or_else(|e| e.into_inner()).clone()),
            local_cells: Mutex::new(self.local_cells.lock().unwrap_or_else(|e| e.into_inner()).clone()),
            diagnostics: self.diagnostics.clone(),
            supplied,
            protocol: self.protocol.clone(),
        })
    }
    pub fn wake(&self) -> Arc<Notify> {
        self.diagnostics.wake.clone()
    }
    /// Bind once before admitting this logical Session. Fresh owners have no
    /// protocol services; retained LSP/tool refreshes keep this exact owner.
    pub fn bind_protocol_services(
        &self,
        bridge: Option<Arc<dyn EditClientBridge>>,
        plan: Arc<dyn EditPlanPolicy>,
    ) -> anyhow::Result<()> {
        let mut protocol = self.protocol.write().unwrap_or_else(|e| e.into_inner());
        if protocol.plan.is_some() {
            anyhow::bail!("EDIT protocol services already bound to this Session");
        }
        protocol.bridge = bridge;
        protocol.plan = Some(plan);
        Ok(())
    }
    pub fn has_pending(&self) -> bool {
        self.diagnostics.state.pending_count() != 0
    }
    pub fn session_id(&self) -> Option<String> {
        self.identity.read().unwrap_or_else(|e| e.into_inner()).as_ref().map(|i| i.id.clone())
    }
    pub fn local_root(&self) -> Option<PathBuf> {
        self.identity.read().unwrap_or_else(|e| e.into_inner()).as_ref().map(|i| i.local_root.clone())
    }
    /// Direct file-resource boundary for a bound logical Session. Snapshot the
    /// actual SessionArtifacts root before awaiting; no ambient registry root.
    /// Root and directory handlers stay outside this bounded file API.
    pub async fn resolve_local_file_resource(
        &self,
        href: &str,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<Option<crate::mcp_native::local::LocalFileResource>, String> {
        if cancel.is_cancelled() { return Err("local:// resource cancelled".into()); }
        let root = self.local_root().ok_or_else(|| "No session - local:// unavailable".to_owned())?;
        let plan = crate::mcp_native::local::LocalPlan::for_session_root(root, href.to_owned());
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err("local:// resource cancelled".into()),
            result = crate::mcp_native::local::resolve_file_resource(plan) => result,
        }
    }
    pub fn local_routes(&self, prepared: &PreparedNativeSession) -> anyhow::Result<LocalRoutes> {
        let heap = prepared.values().heap();
        let cell = heap.object()?;
        if let Some(identity) = self.identity.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
            set_local_cell(&cell, identity)?;
        }
        let options = heap.object()?;
        for (name, key) in [("getArtifactsDir", "artifacts"), ("getSessionId", "id")] {
            let function = heap.fresh_compiled_function(&LOCAL_CALLBACK, &[cell.clone(), heap.string(key.into())?])?;
            heap.define_own_data(&options, name.into(), function, &NoRuntime)?;
        }
        self.local_cells.lock().unwrap_or_else(|e| e.into_inner()).push(cell);
        Ok(LocalRoutes { override_options: Some(options), main_session_manager: None })
    }
    /// Fixed drainLazy: freshness is evaluated when Core invokes the factory.
    /// All fields are authored Host input, rendered by the existing Native
    /// prompt module when installed, then journaled by the ordinary sink.
    pub fn asides(
        &self,
        projector: Arc<AgentMessageProjector>,
        prepared: Option<Arc<PreparedNativeSession>>,
    ) -> Vec<AsideMessage> {
        if !self.has_pending() {
            return Vec::new();
        }
        let batch = Arc::new(Mutex::new(Some(self.diagnostics.state.drain_lazy())));
        vec![AsideMessage::Lazy(Arc::new(move || {
            let Some(batch) = batch.lock().unwrap_or_else(|e| e.into_inner()).take() else {
                return Ok(None);
            };
            let entries = batch.survivors();
            if entries.is_empty() {
                return Ok(None);
            }
            let message = build_late(&entries, &projector, prepared.as_deref())?;
            // Fixed enqueue() late entries have no external receipt to ACK.
            // Keep the native idempotent ephemeral claim callbacks, distinct
            // from the sink's later durable MessageEnd frontier.
            let settled = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let committed = settled.clone();
            message.set_aside_callbacks(AsideCallbacks {
                commit: Some(Arc::new(move || {
                    committed.swap(true, std::sync::atomic::Ordering::AcqRel);
                    Ok(())
                })),
                discard: Some(Arc::new(move |_| {
                    settled.swap(true, std::sync::atomic::Ordering::AcqRel);
                    Ok(())
                })),
            });
            Ok(Some(message))
        }))]
    }
    /// Idle consumption still uses the lazy factory at the serial dispatch
    /// boundary. A build failure propagates; no new conversation/replay occurs.
    pub fn idle_message(
        &self,
        projector: Arc<AgentMessageProjector>,
        prepared: Option<Arc<PreparedNativeSession>>,
    ) -> Result<Option<AgentMessage>, AsideError> {
        match self.asides(projector, prepared).into_iter().next() {
            Some(AsideMessage::Lazy(factory)) => factory(),
            Some(AsideMessage::Ready(message)) => Ok(Some(message)),
            None => Ok(None),
        }
    }
}
/// Pure READ alias resolver: fixed parse.ts53-100 and local-protocol.ts212-255,
/// 299-317 at 596f2da. The Host already bound the root with session_root.
/// It deliberately does not use the plan/write resolver's slash normalization:
/// local:/name has no raw host and is decoded once; local://name has two stages.
/// MIT / Copyright (c) 2025-2026 Can Bölük; 2026 Stencil Labs, Inc.
pub fn resolve_read_plan_reference(local_root: &Path, input: &str) -> Result<PathBuf, String> {
    if !input.starts_with("local:") { return Err("Plan reference is not local:".into()); }
    let combined = if let Some(tail) = input.strip_prefix("local://") {
        // Fixed hierarchical regex preserves raw pathname even when URL parsing
        // fails (for example a namespaced raw host with a nonnumeric port).
        let tail = &tail[..tail.find(['?', '#']).unwrap_or(tail.len())];
        let (host, pathname) = tail.find('/').map_or((tail, ""), |at| (&tail[..at], &tail[at..]));
        let host = decode_plan_reference_component(host).unwrap_or_else(|_| host.to_owned());
        if host.is_empty() {
            if pathname.is_empty() || pathname == "/" { String::new() } else { pathname[1..].to_owned() }
        } else if pathname.is_empty() || pathname == "/" {
            host
        } else {
            format!("{host}{pathname}")
        }
    } else {
        // Fixed new URL(input).pathname for single-slash and opaque forms.
        // extractRelativePath slices its first code unit even for local:foo.
        let parsed = reqwest::Url::parse(input).map_err(|error| error.to_string())?;
        let pathname = parsed.path();
        if pathname.is_empty() || pathname == "/" { String::new() } else {
            pathname.get(1..).ok_or_else(|| "Invalid local: pathname boundary".to_owned())?.to_owned()
        }
    };
    let relative = decode_plan_reference_component(&combined.replace('\\', "/"))
        .map_err(|_| format!("Invalid URL encoding in local:// path: {input}"))?;
    ara_tools::internal_urls::validate_relative_path(&relative)
        .map_err(|error| error.replace("skill://", "local://"))?;
    let root = ara_tools::normalize(&std::path::absolute(local_root).map_err(|error| error.to_string())?);
    // Node path.resolve(root, "D:plan.md") keeps a same-device cwd root.
    // PathBuf::join would replace it with a drive-relative path. Fixed
    // ensureWithinRoot compares strings, so even a same-drive case change is
    // rejected. Preserve the raw prefix character: Rust Disk may normalize it.
    #[cfg(windows)]
    let relative = {
        use std::path::{Component, Prefix};
        let path = Path::new(&relative);
        match path.components().next() {
            Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_)) && !path.has_root() => {
                let root_prefix = match root.components().next() {
                    Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_)) => prefix,
                    _ => return Err("local:// URL escapes local root".into()),
                };
                let root_drive = root_prefix.as_os_str().to_str().and_then(|prefix| prefix.as_bytes().first()).copied();
                if root_drive != relative.as_bytes().first().copied() {
                    return Err("local:// URL escapes local root".into());
                }
                relative[2..].to_owned()
            }
            _ => relative,
        }
    };
    let target = ara_tools::normalize(&root.join(relative));
    if !target.starts_with(&root) { return Err("local:// URL escapes local root".into()); }
    Ok(target)
}
// Same strict percent/UTF8 decoder as the stable binary plan resolver; kept at
// the existing library boundary so this pure READ path can be exercised by the
// Host integration group without a new binary unit-test target.
fn decode_plan_reference_component(input: &str) -> Result<String, ()> {
    let mut bytes = Vec::with_capacity(input.len());
    let source = input.as_bytes();
    let mut offset = 0;
    while offset < source.len() {
        if source[offset] == b'%' {
            let hex = |value: Option<&u8>| value.and_then(|value| char::from(*value).to_digit(16));
            let (Some(high), Some(low)) = (hex(source.get(offset + 1)), hex(source.get(offset + 2))) else { return Err(()); };
            bytes.push((high * 16 + low) as u8);
            offset += 3;
        } else { bytes.push(source[offset]); offset += 1; }
    }
    String::from_utf8(bytes).map_err(|_| ())
}

impl ara_tools::read::ReadPlanReferencePort for HostEditSession {
    fn read_reference_snapshot(&self) -> Result<ara_tools::read::ReadPlanReferenceSnapshot, ToolError> {
        let plan=self.protocol.read().unwrap_or_else(|e|e.into_inner()).plan.clone();
        if let Some(plan)=plan { return plan.read_reference_snapshot(); }
        let native_local_fallback=self.local_root().map(|root|root.join("PLAN.md"));
        Ok(ara_tools::read::ReadPlanReferenceSnapshot::Uri {
            authored:"local://PLAN.md".into(), native_local_fallback,
        })
    }
    fn read_reference_path(&self) -> Option<PathBuf> {
        // Drop the protocol lock before querying the independently owned policy.
        let plan = self.protocol.read().unwrap_or_else(|e| e.into_inner()).plan.clone();
        match plan {
            Some(plan) => plan.read_reference_path(),
            // Fixed AgentSession initializes its reference to local://PLAN.md.
            None => self.local_root().map(|root| root.join("PLAN.md")),
        }
    }
}
impl EditHostPort for HostEditSession {
    fn resolve_edit_native_url(&self,url:&str)->Result<Option<ara_edit::path_policy::UrlResolution>,ara_agent::ToolError> {
        let scheme=ara_tools::internal_urls::hierarchical_scheme(url).unwrap_or_default();
        if scheme.eq_ignore_ascii_case("local") {
            let root=self.local_root().ok_or_else(||ara_agent::ToolError("No session - local:// unavailable".into()))?;
            let native=format!("local://{}",url.split_once("://").map(|(_,tail)|tail).unwrap_or_default());
            let path=resolve_read_plan_reference(&root,&native).map_err(ara_agent::ToolError)?;
            ara_tools::edit_urls::validate_containment(&path,&root)?;
            return Ok(Some(ara_edit::path_policy::UrlResolution {absolute:Some(path),error:None,plan_writable:true}));
        }
        ara_tools::edit_urls::resolve_native_file(&self.path_policy(),url)
    }
    fn enable_lsp(&self) -> bool {
        self.supplied.enable_lsp.unwrap_or(true)
    }
    fn path_policy(&self) -> EditHostPathPolicy {
        let identity = self.identity.read().unwrap_or_else(|e| e.into_inner());
        let protocol = self.protocol.read().unwrap_or_else(|e| e.into_inner());
        let plan_file_path = protocol.plan.as_ref().and_then(|plan| plan.current_plan_path());
        EditHostPathPolicy {
            home_dir: Some(self.home.clone()),
            local_sandbox_root: identity.as_ref().map(|i| i.local_root.clone()),
            vault_roots: self.supplied.vault.as_ref().and_then(|vault| vault.cached_roots()),
            plan_active: plan_file_path.is_some(),
            plan_file_path,
        }
    }
    fn lsp_options(&self) -> EditLspOptions {
        self.supplied.lsp_options
    }
    fn write_lsp_options(&self) -> EditLspOptions {
        self.supplied.write_lsp_options.unwrap_or(self.supplied.lsp_options)
    }
    fn client_bridge(&self) -> Option<Arc<dyn EditClientBridge>> {
        let protocol = self.protocol.read().unwrap_or_else(|e| e.into_inner());
        if protocol.plan.is_some() { protocol.bridge.clone() } else { self.supplied.client_bridge.clone() }
    }
    fn lsp_writethrough(&self) -> Option<Arc<dyn EditLspWritethrough>> {
        self.supplied.lsp.clone()
    }
    fn file_mutation_fallbacks(&self) -> Option<Arc<ara_tools::edit_file_fallback::EditFileMutationFallbackRegistry>> {
        self.supplied.file_fallbacks.as_ref().filter(|r| r.has_write_fallback() || r.has_delete_fallback()).cloned()
    }
    fn file_mutation_session_id(&self) -> Option<String> {
        self.session_id()
    }
    fn deferred_diagnostics_host(&self) -> Option<Arc<dyn EditDeferredDiagnosticsHost>> {
        Some(self.diagnostics.clone())
    }
    fn diagnostics_host(&self) -> Option<Arc<dyn EditDeferredDiagnosticsHost>> {
        Some(self.diagnostics.clone())
    }
    fn bump_file_mutation_version(&self, path: &Path) {
        self.diagnostics.bump_file_mutation_version(path);
    }
    fn invalidate_after_write(&self, path: &Path) {
        ara_tools::engine::invalidate_fs_scan_cache(Some(path));
    }
    fn invalidate_after_delete(&self, path: &Path) {
        ara_tools::engine::invalidate_fs_scan_cache(Some(path));
    }
    fn invalidate_after_rename(&self, source: &Path, destination: &Path) {
        ara_tools::engine::invalidate_fs_scan_cache(Some(source));
        if source != destination {
            ara_tools::engine::invalidate_fs_scan_cache(Some(destination));
        }
    }
}
fn set_local_cell(cell: &ValueHandle, identity: &Identity) -> Result<(), ValueError> {
    let heap = cell.heap();
    let artifacts = match identity.artifacts.directory() {
        Some(path) => heap.string(path.to_string_lossy().as_ref().into())?,
        None => heap.undefined()?,
    };
    heap.define_own_data(cell, "artifacts".into(), artifacts, &NoRuntime)?;
    heap.define_own_data(cell, "id".into(), heap.string(identity.id.as_str().into())?, &NoRuntime)
}
fn local_callback(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    _: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    let key = heap.to_string(&captures[1], runtime)?;
    heap.get(&captures[0], &key.into(), runtime)
}
static LOCAL_CALLBACK: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Host Session LocalProtocolOptions callback",
    name: "",
    length: 0,
    form: CompiledFunctionForm::Arrow,
    capture_count: 2,
    body: local_callback,
};
fn build_late(
    entries: &[EditDeferredDiagnosticEntry],
    projector: &AgentMessageProjector,
    prepared: Option<&PreparedNativeSession>,
) -> Result<AgentMessage, AsideError> {
    let files = entries
        .iter()
        .map(|entry| {
            json!({"path":entry.path.to_string_lossy(),"summary":entry.summary,
        "errored":entry.errored,"messages":entry.messages})
        })
        .collect::<Vec<_>>();
    let context = json!({"multiple":files.len()>1,"files":files});
    let fail = |error: &dyn std::fmt::Display| AsideError(format!("late LSP diagnostics: {error}"));
    if let Some(prepared) = prepared {
        let values = prepared.values();
        let heap = values.heap();
        let rt = values.runtime();
        let prompt = prepared.prompt().prompt();
        let render = heap.get(prompt, &"render".into(), rt).map_err(|e| fail(&e))?;
        let content = heap
            .call(
                &render,
                prompt,
                &[
                    heap.string(LATE_TEMPLATE.into()).map_err(|e| fail(&e))?,
                    heap.import_serde(&context).map_err(|e| fail(&e))?,
                ],
                rt,
            )
            .map_err(|e| fail(&e))?;
        let body = heap
            .import_serde(&json!({"role":"custom","customType":LATE_TYPE,"display":true,
            "attribution":"agent","details":{"files":files}}))
            .map_err(|e| fail(&e))?;
        heap.define_own_data(&body, "content".into(), content, rt).map_err(|e| fail(&e))?;
        let date = prepared.prompt().date();
        let now = heap.get(date, &"now".into(), rt).map_err(|e| fail(&e))?;
        let timestamp = heap.call(&now, date, &[], rt).map_err(|e| fail(&e))?;
        heap.define_own_data(&body, "timestamp".into(), timestamp, rt).map_err(|e| fail(&e))?;
        AgentMessage::from_graph(body, Some(Arc::new(json!({"origin":LATE_TYPE,"attribution":"agent"}))))
            .map_err(|e| fail(&e))
    } else {
        let content = ara_prompt::render(LATE_TEMPLATE, &context).map_err(|e| fail(&e))?;
        projector
            .new_custom_input(json!({"role":"custom","customType":LATE_TYPE,"content":content,"display":true,
            "attribution":"agent","details":{"files":files},"timestamp":chrono::Utc::now().timestamp_millis()}))
            .map_err(|e| fail(&e))
    }
}

/// Expanded observation inside the existing original builtin EDIT family.
#[cfg(test)]
pub(crate) fn observe_fixed_session_host_family() {
    use ara_tools::edit_host::EditDeferredDiagnostics;
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("real-home");
    let host = HostEditSession::new(home.clone(), Default::default());
    let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
    let artifacts = SessionArtifacts::for_journal(&journal);
    let prepared = PreparedNativeSession::prepare().unwrap();
    let routes = host.local_routes(&prepared).unwrap();
    host.bind(&journal, artifacts.clone()).unwrap();
    let policy = host.path_policy();
    assert_eq!(policy.home_dir, Some(home));
    assert_eq!(
        policy.local_sandbox_root,
        Some(crate::mcp_native::local::session_root(artifacts.directory(), journal.session_id()).unwrap())
    );
    assert!(policy.vault_roots.is_none() && policy.plan_file_path.is_none());
    assert!(
        host.client_bridge().is_none() && host.lsp_writethrough().is_none() && host.file_mutation_fallbacks().is_none()
    );
    let values = prepared.values();
    let heap = values.heap();
    let options = routes.override_options.as_ref().unwrap();
    let get_id = heap.get(options, &"getSessionId".into(), values.runtime()).unwrap();
    let id = heap.call(&get_id, options, &[], values.runtime()).unwrap();
    assert_eq!(heap.export_serde(&id).unwrap(), json!(journal.session_id()));
    let get_directory = heap.get(options, &"getArtifactsDir".into(), values.runtime()).unwrap();
    let actual = heap.call(&get_directory, options, &[], values.runtime()).unwrap();
    assert_eq!(heap.export_serde(&actual).unwrap(), json!(artifacts.directory().unwrap().to_string_lossy()));
    let projector = Arc::new(AgentMessageProjector::default());
    projector
        .bind_native_session(
            &mut journal,
            Arc::new(crate::actual_session::GraphJournalEncoder::new(values.clone(), None).unwrap()),
        )
        .unwrap();
    let diagnostics = host.deferred_diagnostics_host().unwrap();
    let deferred = EditDeferredDiagnostics::new(diagnostics.clone(), true);
    let path = directory.path().join("first.rs");
    let old = deferred.begin(&path);
    old.finalize(None);
    let fresh = deferred.begin(&path);
    assert!(old.signal().is_cancelled());
    let raw = json!({"summary":"1 error(s)","errored":true,"messages":["first.rs:1:2 [error] exact"]});
    assert!(!old.on_deferred_diagnostics(raw.clone()).unwrap());
    fresh.finalize(None);
    assert!(fresh.on_deferred_diagnostics(raw.clone()).unwrap());
    let lazy = host.asides(projector.clone(), Some(prepared.clone()));
    assert!(!host.has_pending());
    diagnostics.bump_file_mutation_version(&path);
    match &lazy[0] {
        AsideMessage::Lazy(factory) => assert!(factory().unwrap().is_none()),
        _ => panic!("expected lazy"),
    }
    let repeated = deferred.begin(&path);
    repeated.finalize(None);
    assert!(!repeated.on_deferred_diagnostics(raw).unwrap(), "same Session ledger deduplicates a repeated issue");
    diagnostics.reduce_diagnostics(&path, json!({"summary":"no issues","errored":false,"messages":[]})).unwrap();
    for (file, severity) in [("first.rs", "error"), ("second.rs", "warning")] {
        let target = directory.path().join(file);
        let handle = deferred.begin(&target);
        handle.finalize(None);
        assert!(
            handle
                .on_deferred_diagnostics(json!({"summary":format!("1 {severity}(s)"),
            "errored":severity=="error","messages":[format!("{file}:1:2 [{severity}] exact")]}))
                .unwrap()
        );
    }
    let message = host.idle_message(projector.clone(), Some(prepared.clone())).unwrap().unwrap();
    assert!(matches!(message.body_snapshot(), ara_agent::AgentMessageBody::Graph(_)));
    let body = message.json().unwrap();
    assert_eq!(body["customType"], LATE_TYPE);
    assert_eq!(body["role"], "custom");
    assert_eq!(body["attribution"], "agent");
    assert_eq!(body["display"], true);
    assert_eq!(body["details"]["files"].as_array().unwrap().len(), 2);
    assert!(body["content"].as_str().unwrap().contains("Late LSP diagnostics arrived for 2 files"));
    let entry_id = projector.append_current_body(&mut journal, &message).unwrap();
    let entry = journal.entries().iter().find(|entry| entry.id == entry_id).unwrap();
    assert!(entry.native_message.is_some(), "same Native body reaches the actual journal frontier");

    // The original Host family exercises this direct Native prompt dependency;
    // each call keeps the exact fixed template and a fresh actual Graph context.
    let prompt = prepared.prompt().prompt();
    let render = heap.get(prompt, &"render".into(), values.runtime()).unwrap();
    for context in [
        json!({"multiple":false,"files":[{"path":"one<&>.rs","summary":"1 <error> & ` = '",
            "messages":["one:first <&>","one:second"]}]}),
        json!({"multiple":true,"files":[{"path":"first.rs","summary":"first-summary",
            "messages":["first:a","first:b"]},{"path":"second.rs","summary":"second-summary",
            "messages":["second:a"]}]}),
        json!({"multiple":false,"files":[]}),
    ] {
        let original_context = heap.import_serde(&context).unwrap();
        let rendered = heap
            .call(&render, prompt, &[heap.string(LATE_TEMPLATE.into()).unwrap(), original_context], values.runtime())
            .unwrap();
        let actual = heap.to_string(&rendered, values.runtime()).unwrap().to_utf8().unwrap();
        let expected = ara_prompt::render(LATE_TEMPLATE, &context).unwrap();
        assert_eq!(
            actual, expected,
            "Native fixed AST lane retains nested this/@last and original noEscape:true output"
        );
        if context["files"].as_array().unwrap().len() == 1 {
            assert!(actual.contains("arrived after the edit returned:"));
            assert!(actual.contains("one<&>.rs") && actual.contains("one:first <&>"));
            assert!(!actual.contains("&lt;") && !actual.contains("&amp;"));
        }
    }
    static ORIGINAL_TEMPLATE_FIELD: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "fixed late template original helper: resolvePath Function.call(context)",
        name: "summary",
        length: 0,
        form: CompiledFunctionForm::Ordinary,
        capture_count: 1,
        body: |heap, captures, receiver, _, runtime| {
            assert!(receiver.same_identity(&captures[0]));
            heap.get(receiver, &"actualSummary".into(), runtime)
        },
    };
    static THROW_TEMPLATE_FIELD: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "fixed late template original helper: field getter original throw",
        name: "messages",
        length: 0,
        form: CompiledFunctionForm::Ordinary,
        capture_count: 1,
        body: |_, captures, _, _, _| Err(ValueError::Thrown(captures[0].clone())),
    };
    let context = heap
        .import_serde(&json!({"multiple":false,"files":[{"path":"safe.rs",
        "actualSummary":"function-summary","messages":["first","last"]}]}))
        .unwrap();
    let files = heap.get(&context, &"files".into(), values.runtime()).unwrap();
    let file = heap.get(&files, &"0".into(), values.runtime()).unwrap();
    let summary = heap.fresh_compiled_function(&ORIGINAL_TEMPLATE_FIELD, &[file.clone()]).unwrap();
    heap.define_own_data(&file, "summary".into(), summary, values.runtime()).unwrap();
    let safe =
        prepared.prompt().safe_string(heap, &heap.string("<safe&summary>".into()).unwrap(), values.runtime()).unwrap();
    heap.define_own_data(&file, "actualSummary".into(), safe, values.runtime()).unwrap();
    let rendered = heap
        .call(&render, prompt, &[heap.string(LATE_TEMPLATE.into()).unwrap(), context.clone()], values.runtime())
        .unwrap();
    assert!(heap.to_string(&rendered, values.runtime()).unwrap().to_utf8().unwrap().contains("<safe&summary>"));
    let reason = heap.object().unwrap();
    let getter = heap.fresh_compiled_function(&THROW_TEMPLATE_FIELD, &[reason.clone()]).unwrap();
    let ara_rpc::value_heap::ValueSnapshot::Function(getter) = heap.snapshot(&getter).unwrap() else {
        panic!("original compiled getter expected")
    };
    heap.define_own(
        &file,
        "messages".into(),
        ara_rpc::value_heap::PropertyDescriptor::Accessor {
            get: Some(getter),
            set: None,
            enumerable: true,
            configurable: true,
        },
        values.runtime(),
    )
    .unwrap();
    assert!(matches!(heap.call(&render,prompt,&[heap.string(LATE_TEMPLATE.into()).unwrap(),context],values.runtime()),
        Err(ValueError::Thrown(original)) if original.same_identity(&reason)));
    let replacement = host.fresh();
    assert_eq!(replacement.deferred_diagnostics_host().unwrap().get_file_mutation_version(&path), 0);
    assert!(!replacement.has_pending() && replacement.session_id().is_none());
}
