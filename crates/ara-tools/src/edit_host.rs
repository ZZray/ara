//! Host ports for fixed OMP 596f2da EditTool file mutation (MIT).
//! Sources: edit/index.ts, tools/acp-bridge.ts, session/client-bridge.ts,
//! lsp/writethrough.ts. These ports do not implement ACP transport or an LSP
//! client/server. Hosts must supply those services; the Core owns edit state.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak, atomic::{AtomicBool, Ordering}};

use async_trait::async_trait;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

/// Resolved current Session policy; storage location and plan state are Host-owned.
#[derive(Clone, Debug, Default)]
pub struct EditHostPathPolicy {
    pub home_dir: Option<PathBuf>,
    pub local_sandbox_root: Option<PathBuf>,
    pub vault_roots: Option<Vec<(String, PathBuf)>>,
    pub plan_active: bool,
    pub plan_file_path: Option<String>,
}

impl EditHostPathPolicy {
    /// Same resolved policy for EDIT and the ordinary WRITE filesystem branch.
    pub fn file_policy(&self, cwd: &Path) -> ara_edit::PathPolicy {
        ara_edit::PathPolicy {
            cwd: cwd.to_owned(),
            home_dir: self.home_dir.clone().unwrap_or_default(),
            url_schemes: vec!["local".into(), "vault".into()],
            url_alias_schemes: vec!["local".into()],
            plan_writable_roots: self.local_sandbox_root.iter().cloned().collect(),
            plan_active: self.plan_active,
            block_auto_generated: false,
        }
    }
}

/// Fixed acp-bridge.ts. The same destination controls policy and routing.
pub fn should_route_write_through_bridge(
    host: &dyn EditHostPort,
    cwd: &Path,
    requested: &str,
    absolute: &Path,
) -> bool {
    let state = host.path_policy();
    let policy = state.file_policy(cwd);
    if policy.is_internal_url(requested)
        || crate::internal_urls::hierarchical_scheme(requested.strip_prefix('@').unwrap_or(requested)).is_some_and(|scheme|!scheme.eq_ignore_ascii_case("file")) { return false; }
    // Use the new physical sandbox check through plan authorization, without
    // changing the current session's plan-mode state.
    let mut sandbox_policy = policy.clone();
    sandbox_policy.plan_active = true;
    if sandbox_policy.enforce_write(&absolute.to_string_lossy(), ara_edit::FileOp::Update, None, &HashMap::new()).is_ok() {
        return false;
    }
    !(state.plan_active && state.plan_file_path.as_ref().is_some_and(|plan| {
        policy.is_internal_url(plan)
            && host.resolve_edit_native_url(plan).is_ok_and(|answer| answer.is_some_and(|answer| answer.error.is_none() && answer.absolute.as_deref()==Some(absolute)))
    }))
}

pub struct BridgeWriteFailure {
    pub error: std::io::Error,
    /// Admission is not proof that the client's external write was absent.
    pub admitted: bool,
    pub acknowledged: bool,
}

/// No cancellation race after client admission, no fallback/replay on failure.
/// Success returns best-effort readback, as in the fixed ACP helper.
pub async fn route_write_through_bridge(
    host: &dyn EditHostPort,
    cwd: &Path,
    requested: &str,
    absolute: &Path,
    content: &str,
    cancel: Option<CancellationToken>,
    deferred: Option<&Arc<EditDeferredDiagnostics>>,
) -> Result<Option<String>, BridgeWriteFailure> {
    if !should_route_write_through_bridge(host, cwd, requested, absolute) {
        return Ok(None);
    }
    let Some(bridge) = host.client_bridge().filter(|bridge| bridge.can_write_text_file()) else {
        return Ok(None);
    };
    let change_type = if absolute.exists() { EditFileChangeType::Changed } else { EditFileChangeType::Created };
    if cancel.as_ref().is_some_and(CancellationToken::is_cancelled) {
        return Err(BridgeWriteFailure {
            error: std::io::Error::new(std::io::ErrorKind::Interrupted, "Aborted: Cancelled"),
            admitted: false,
            acknowledged: false,
        });
    }
    bridge.write_text_file(absolute, content).await.map_err(|error| BridgeWriteFailure {
        error,
        admitted: true,
        acknowledged: false,
    })?;
    if host.enable_lsp()
        && let Some(lsp) = host.lsp_writethrough()
    {
        lsp.notify_watched_files(cwd, &[EditFileChange { path: absolute.to_owned(), change_type }], cancel)
            .await
            .map_err(|error| BridgeWriteFailure { error, admitted: true, acknowledged: true })?;
    }
    host.invalidate_after_write(absolute);
    if let Some(deferred) = deferred {
        deferred.bump_file_mutation_version(absolute);
    } else {
        host.bump_file_mutation_version(absolute);
    }
    Ok(Some(
        std::fs::read(absolute)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_else(|_| content.to_owned()),
    ))
}

/// Current Host-resolved LSP leaves, read at each admitted mutation call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditLspOptions {
    pub enable_format: bool,
    pub enable_diagnostics: bool,
    pub deduplicate_diagnostics: bool,
}

impl Default for EditLspOptions {
    fn default() -> Self {
        // Fixed settings-schema.ts:3822/3844/3855. EditTool gates dedup with
        // enable_diagnostics after reading these Host-resolved leaves.
        Self { enable_format: false, enable_diagnostics: false, deduplicate_diagnostics: true }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditLspBatch {
    pub id: String,
    pub flush: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditFileChangeType {
    Created,
    Changed,
    Deleted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditFileChange {
    pub path: PathBuf,
    pub change_type: EditFileChangeType,
}

/// The capability bit is separate from method availability. An absent method
/// is represented by the Host returning no bridge; it must not be synthesized.
#[async_trait]
pub trait EditClientBridge: Send + Sync {
    fn can_write_text_file(&self) -> bool;

    /// Read capability is independent of write capability. Existing write-only
    /// Hosts remain write-only until they advertise and supply this method.
    fn can_read_text_file(&self) -> bool {
        false
    }

    /// Return the complete editor buffer. Core applies selectors once, locally.
    async fn read_text_file(&self, _absolute_path: &Path) -> std::io::Result<String> {
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "Client does not supply readTextFile"))
    }

    /// ACP filesystem writes have no cancellation message. Core checks the
    /// token before admission, then awaits settlement without racing a token.
    /// Returning success means the client's write settled, not byte equality.
    async fn write_text_file(&self, absolute_path: &Path, content: &str) -> std::io::Result<()>;
}

pub struct EditLspWriteRequest<'a> {
    pub cwd: &'a Path,
    pub absolute_path: &'a Path,
    pub content: &'a str,
    pub options: EditLspOptions,
    pub batch: Option<EditLspBatch>,
    pub cancel: Option<CancellationToken>,
    /// Consult only when LSP post-processing starts, including batch flush.
    /// The fixed edit callback returns a handle only for this request's path.
    pub get_deferred: Option<EditDeferredDiagnosticsFactory>,
    /// Apply to fresh inline diagnostics per file, before merging a batch.
    /// Late diagnostics are raw: their sink uses the same Session ledger.
    pub transform_diagnostics: Option<EditDiagnosticsTransform>,
    /// The same process-shared ordinary byte-write fallback used by EditTool.
    /// The service must use this primitive for its actual content commits.
    pub file_fallbacks: Option<Arc<crate::edit_file_fallback::EditFileMutationFallbackRegistry>>,
    pub mutation_session_id: Option<String>,
}

/// The LSP service owns formatting, server/document-version freshness and
/// shared edit/write batches. Core supplies the late sink and mutation-version
/// check through the request callbacks. A write must
/// commit its bytes now; a batch may defer only LSP post-processing. Flush
/// must read the latest file, not replay an old whole-file snapshot.
/// Rich return for target b91db66e. None preserves a legacy Host's request-body
/// contract; the real supplier returns Some with its settled formatter bytes.
#[derive(Clone, Debug, Default)]
pub struct EditLspWritethroughResult {
    pub diagnostics: Option<Value>,
    pub final_content: Option<String>,
}

#[async_trait]
pub trait EditLspWritethrough: Send + Sync {
    async fn write_through(&self, request: EditLspWriteRequest<'_>) -> std::io::Result<Option<Value>>;

    /// Existing Host implementations remain source-compatible. New consumers
    /// select this method; a production supplier must return its final bytes.
    async fn write_through_result(&self, request: EditLspWriteRequest<'_>) -> std::io::Result<EditLspWritethroughResult> {
        Ok(EditLspWritethroughResult { diagnostics: self.write_through(request).await?, final_content: None })
    }

    async fn notify_watched_files(
        &self,
        cwd: &Path,
        changes: &[EditFileChange],
        cancel: Option<CancellationToken>,
    ) -> std::io::Result<()>;

    async fn flush_batch(
        &self,
        id: &str,
        cwd: &Path,
        cancel: Option<CancellationToken>,
    ) -> std::io::Result<Option<Value>>;
}

/// Session facade. The Host supplies existing services and mutation counters;
/// Core does not choose a product, artifacts directory or transport.
pub trait EditHostPort: Send + Sync {
    /// Native file schemes only. Explicit registered URI ports take precedence.
    /// Production Hosts may reuse their real protocol parser/containment here.
    fn resolve_edit_native_url(&self, url: &str) -> Result<Option<ara_edit::path_policy::UrlResolution>, crate::ToolError> {
        crate::edit_urls::resolve_native_file(&self.path_policy(), url)
    }
    fn path_policy(&self) -> EditHostPathPolicy {
        EditHostPathPolicy::default()
    }

    fn enable_lsp(&self) -> bool {
        true
    }

    fn lsp_options(&self) -> EditLspOptions {
        EditLspOptions::default()
    }

    /// The production Host reads diagnosticsOnWrite independently. Existing
    /// Hosts keep their explicit shared options unless they supply this leaf.
    fn write_lsp_options(&self) -> EditLspOptions {
        self.lsp_options()
    }

    fn client_bridge(&self) -> Option<Arc<dyn EditClientBridge>> {
        None
    }

    fn lsp_writethrough(&self) -> Option<Arc<dyn EditLspWritethrough>> {
        None
    }

    /// Session-wide inline reduction ledger, independent of late delivery.
    /// Existing Hosts keep their shared owner; a Host without a late queue
    /// supplies its existing ledger here before constructing WriteTool.
    fn diagnostics_host(&self) -> Option<Arc<dyn EditDeferredDiagnosticsHost>> {
        self.deferred_diagnostics_host()
    }

    /// Install before constructing EditTool. None means late delivery is not
    /// installed; an LSP writer must use its ordinary inline wait in that case.
    fn deferred_diagnostics_host(&self) -> Option<Arc<dyn EditDeferredDiagnosticsHost>> {
        None
    }

    /// Process-wide handlers, explicitly installed by the Host. No handler is
    /// synthesized and no approval or permission is inferred by this method.
    fn file_mutation_fallbacks(&self) -> Option<Arc<crate::edit_file_fallback::EditFileMutationFallbackRegistry>> {
        None
    }

    /// Issuing Session identity captured at the admitted tool call. External
    /// callers have None; it must never borrow a registering handler's identity.
    fn file_mutation_session_id(&self) -> Option<String> {
        None
    }

    fn invalidate_after_write(&self, _path: &Path) {}
    fn invalidate_after_delete(&self, _path: &Path) {}
    fn invalidate_after_rename(&self, _source: &Path, _destination: &Path) {}
    fn bump_file_mutation_version(&self, _path: &Path) {}
}

pub type EditDeferredDiagnosticsFactory = Arc<dyn Fn(&Path) -> Option<EditDeferredDiagnosticsHandle> + Send + Sync>;
pub type EditDiagnosticsTransform = Arc<dyn Fn(&Path, Value) -> std::io::Result<Value> + Send + Sync>;

/// Required Session services from fixed sdk.ts:1763-66,1853,1862-67. All
/// mutation tools must share this owner. These methods have no stub defaults.
pub trait EditDeferredDiagnosticsHost: Send + Sync {
    fn bump_file_mutation_version(&self, path: &Path) -> u64;
    fn get_file_mutation_version(&self, path: &Path) -> u64;
    fn reduce_diagnostics(&self, path: &Path, result: Value) -> std::io::Result<Value>;
    fn queue_deferred_diagnostics(&self, entry: EditDeferredDiagnosticEntry) -> std::io::Result<()>;
}

/// Fixed ToolSession.DeferredDiagnosticsEntry. The stale predicate is evaluated
/// at injection, after queueing or lazy draining, rather than cached on arrival.
#[derive(Clone)]
pub struct EditDeferredDiagnosticEntry {
    pub path: PathBuf,
    pub summary: String,
    pub messages: Vec<String>,
    pub errored: bool,
    mutation_version: u64,
    owner: Weak<dyn EditDeferredDiagnosticsHost>,
}

impl EditDeferredDiagnosticEntry {
    pub fn is_stale(&self) -> bool {
        self.owner.upgrade().is_none_or(|host| host.get_file_mutation_version(&self.path) != self.mutation_version)
    }
}

/// A real in-memory Session owner for the narrow map/ledger/queue contract.
/// Installing it does not install an LSP process, a yield dispatcher, idle
/// scheduling, transcript persistence or model injection; the Host owns those.
#[derive(Default)]
pub struct EditDiagnosticsSession {
    versions: Mutex<HashMap<PathBuf, u64>>,
    ledger: Mutex<HashMap<PathBuf, HashSet<String>>>,
    queue: Mutex<VecDeque<EditDeferredDiagnosticEntry>>,
}

impl EditDiagnosticsSession {
    pub fn pending_count(&self) -> usize {
        self.queue.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Fixed YieldQueue.drainLazy: drain now, evaluate staleness only when the
    /// Host builds/injects the batch. Returning entries is not model delivery.
    pub fn drain_lazy(&self) -> EditLateDiagnosticsBatch {
        EditLateDiagnosticsBatch { entries: self.queue.lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect() }
    }
}

pub struct EditLateDiagnosticsBatch {
    entries: Vec<EditDeferredDiagnosticEntry>,
}

impl EditLateDiagnosticsBatch {
    pub fn survivors(self) -> Vec<EditDeferredDiagnosticEntry> {
        self.entries.into_iter().filter(|entry| !entry.is_stale()).collect()
    }
}

fn diagnostic_messages(result: &Value) -> std::io::Result<Vec<String>> {
    let invalid = || std::io::Error::new(std::io::ErrorKind::InvalidData, "LSP diagnostics.messages must be strings");
    let messages = result.get("messages").and_then(Value::as_array).ok_or_else(invalid)?;
    messages.iter().map(|message| message.as_str().map(str::to_owned).ok_or_else(invalid)).collect()
}

/// Fixed diagnostics-ledger.ts /^.*?:\d+:\d+\s+/ and utils.ts:272-291.
fn diagnostic_identity(message: &str) -> &str {
    static LOCATION: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r"^[^\r\n\u{2028}\u{2029}]*?:[0-9]+:[0-9]+[\t-\r \u{00A0}\u{1680}\u{2000}-\u{200A}\u{2028}\u{2029}\u{202F}\u{205F}\u{3000}\u{FEFF}]+"
        ).unwrap()
    });
    LOCATION.find(message).map_or(message, |prefix| &message[prefix.end()..])
}

fn diagnostic_summary(messages: &[String]) -> (String, bool) {
    static SEVERITY: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"(?i)\[(error|warning|info|hint)\]").unwrap());
    let mut counts = [0usize; 4];
    let kinds = ["error", "warning", "info", "hint"];
    for message in messages {
        if let Some(found) = SEVERITY.captures(message) {
            let kind = found[1].to_ascii_lowercase();
            let index = kinds.iter().position(|value| *value == kind).unwrap();
            counts[index] += 1;
        }
    }
    let parts = kinds
        .iter()
        .zip(counts)
        .filter(|(_, count)| *count > 0)
        .map(|(kind, count)| format!("{count} {kind}(s)"))
        .collect::<Vec<_>>();
    (if parts.is_empty() { "no issues".into() } else { parts.join(", ") }, counts[0] > 0)
}

impl EditDeferredDiagnosticsHost for EditDiagnosticsSession {
    fn bump_file_mutation_version(&self, path: &Path) -> u64 {
        let mut versions = self.versions.lock().unwrap_or_else(|e| e.into_inner());
        let version = versions.entry(path.to_owned()).or_default();
        *version = version.checked_add(1).expect("file mutation counter exhausted");
        *version
    }

    fn get_file_mutation_version(&self, path: &Path) -> u64 {
        // Zero is only the actual empty-map state, not a missing Host result.
        self.versions.lock().unwrap_or_else(|e| e.into_inner()).get(path).copied().unwrap_or(0)
    }

    fn reduce_diagnostics(&self, path: &Path, mut result: Value) -> std::io::Result<Value> {
        let messages = diagnostic_messages(&result)?;
        let mut seen = self.ledger.lock().unwrap_or_else(|e| e.into_inner());
        let previous = seen.get(path);
        let identities = messages.iter().map(|message| diagnostic_identity(message).to_owned()).collect::<HashSet<_>>();
        let fresh = messages
            .iter()
            .filter(|message| previous.is_none_or(|seen| !seen.contains(diagnostic_identity(message))))
            .cloned()
            .collect::<Vec<_>>();
        if identities.is_empty() {
            seen.remove(path);
        } else {
            seen.insert(path.to_owned(), identities);
        }
        if fresh.len() != messages.len() {
            let (summary, errored) = diagnostic_summary(&fresh);
            result["messages"] = serde_json::json!(fresh);
            result["summary"] = Value::String(summary);
            result["errored"] = Value::Bool(errored);
        }
        Ok(result)
    }

    fn queue_deferred_diagnostics(&self, entry: EditDeferredDiagnosticEntry) -> std::io::Result<()> {
        self.queue.lock().unwrap_or_else(|e| e.into_inner()).push_back(entry);
        Ok(())
    }
}

/// Fixed deferred-diagnostics.ts state is per mutation-tool instance; the
/// counters and ledger above are Session-wide. Tokens are independent of the
/// finished edit call, and only a newer begin or inline finalization aborts one.
pub struct EditDeferredDiagnostics {
    host: Arc<dyn EditDeferredDiagnosticsHost>,
    deduplicate: Arc<dyn Fn() -> bool + Send + Sync>,
    pending: Mutex<HashMap<PathBuf, CancellationToken>>,
}

impl EditDeferredDiagnostics {
    pub fn new(host: Arc<dyn EditDeferredDiagnosticsHost>, deduplicate: bool) -> Arc<Self> {
        Self::with_live_dedup(host, Arc::new(move || deduplicate))
    }

    /// The reader belongs to the existing Session settings owner. It is read
    /// for inline call preparation and again for late delivery of fetches
    /// already in flight.
    pub fn with_live_dedup(host: Arc<dyn EditDeferredDiagnosticsHost>, deduplicate: Arc<dyn Fn() -> bool + Send + Sync>) -> Arc<Self> {
        Arc::new(Self { host, deduplicate, pending: Mutex::new(HashMap::new()) })
    }

    pub fn begin(self: &Arc<Self>, path: &Path) -> EditDeferredDiagnosticsHandle {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(previous) = pending.remove(path) {
            previous.cancel();
        }
        let mutation_version = self.host.bump_file_mutation_version(path);
        EditDeferredDiagnosticsHandle {
            owner: self.clone(),
            path: path.to_owned(),
            mutation_version,
            signal: CancellationToken::new(),
        }
    }

    pub(crate) fn bump_file_mutation_version(&self, path: &Path) {
        self.host.bump_file_mutation_version(path);
    }

    pub(crate) fn transform(self: &Arc<Self>) -> Option<EditDiagnosticsTransform> {
        if !(self.deduplicate)() { return None; }
        let host = self.host.clone();
        Some(Arc::new(move |path, result| host.reduce_diagnostics(path, result)))
    }

    /// WRITE's fixed callback may service every path in an EDIT/WRITE batch.
    pub(crate) fn for_all_paths_tracking(self: &Arc<Self>, began: Arc<AtomicBool>) -> EditDeferredDiagnosticsFactory {
        let owner = self.clone();
        Arc::new(move |destination| {
            began.store(true, Ordering::Release);
            Some(owner.begin(destination))
        })
    }

    pub(crate) fn for_path_tracking(self: &Arc<Self>, path: &Path, began: Arc<AtomicBool>) -> EditDeferredDiagnosticsFactory {
        let owner = self.clone();
        let path = path.to_owned();
        Arc::new(move |destination| {
            if destination != path.as_path() { return None; }
            began.store(true, Ordering::Release);
            Some(owner.begin(destination))
        })
    }

    pub(crate) fn for_all_paths(self: &Arc<Self>) -> EditDeferredDiagnosticsFactory {
        let owner = self.clone();
        Arc::new(move |destination| Some(owner.begin(destination)))
    }

    pub(crate) fn for_path(self: &Arc<Self>, path: &Path) -> EditDeferredDiagnosticsFactory {
        let owner = self.clone();
        let path = path.to_owned();
        Arc::new(move |destination| (destination == path.as_path()).then(|| owner.begin(destination)))
    }
}

#[derive(Clone)]
pub struct EditDeferredDiagnosticsHandle {
    owner: Arc<EditDeferredDiagnostics>,
    path: PathBuf,
    mutation_version: u64,
    signal: CancellationToken,
}

impl EditDeferredDiagnosticsHandle {
    pub fn signal(&self) -> CancellationToken {
        self.signal.clone()
    }

    /// Call once with the inline result. None retains the pending fetch;
    /// a concrete result cancels it. The LSP writer owns this finalization.
    pub fn finalize(&self, diagnostics: Option<&Value>) {
        if diagnostics.is_none() {
            self.owner.pending.lock().unwrap_or_else(|e| e.into_inner()).insert(self.path.clone(), self.signal.clone());
        } else {
            self.signal.cancel();
        }
    }

    /// Fixed writethrough's late completion checks its deferred signal before
    /// forwarding the raw result, then the sink reduces/queues it. False means
    /// cancellation or dedup suppressed delivery; queue errors stay observable.
    pub fn on_deferred_diagnostics(&self, diagnostics: Value) -> std::io::Result<bool> {
        if self.signal.is_cancelled() {
            return Ok(false);
        }
        self.owner.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.path);
        let deduplicate = (self.owner.deduplicate)();
        let effective = if deduplicate {
            self.owner.host.reduce_diagnostics(&self.path, diagnostics)?
        } else {
            diagnostics
        };
        let messages = diagnostic_messages(&effective)?;
        if deduplicate && messages.is_empty() {
            return Ok(false);
        }
        let summary = match effective.get("summary") {
            None | Some(Value::Null) => String::new(),
            Some(Value::String(summary)) => summary.clone(),
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "LSP diagnostics.summary must be text",
                ));
            }
        };
        let errored = effective.get("errored").and_then(Value::as_bool).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "LSP diagnostics.errored must be boolean")
        })?;
        self.owner.host.queue_deferred_diagnostics(EditDeferredDiagnosticEntry {
            path: self.path.clone(),
            summary,
            messages,
            errored,
            mutation_version: self.mutation_version,
            owner: Arc::downgrade(&self.owner.host),
        })?;
        Ok(true)
    }
}
