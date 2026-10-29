//! Built-in tools (OMP `packages/coding-agent/src/tools/{read,write,bash,grep,glob}.ts`,
//! `exec/`, `session/streaming-output.ts` at
//! 596f2da7101178214aa27a753529d15e6b7ad91d).
//!
//! These tools perform real file and process effects. They are not a
//! sandbox: the host decides which calls may run (`LoopHooks::before_tool_call`)
//! and which directory is the working root.

pub mod ast_edit;
pub mod ast_edit_engine;
pub mod ast_edit_render;
pub mod ast_grep;
pub mod bash;
pub mod conflict_detect;
pub mod default_declarations;
pub mod edit;
pub mod edit_file_fallback;
pub mod edit_host;
pub mod edit_post_apply;
pub mod edit_runtime_policy;
pub mod engine;
pub mod glob;
pub mod grammars;
pub mod grep;
pub mod internal_urls;
pub mod output;
pub mod paths;
mod read_archive_date;
pub use read_archive_date::{ArchiveDatePort, ArchiveLocalDate};
pub mod read;
pub mod read_document;
pub mod read_image;
pub mod read_profile;
pub mod read_sqlite;
pub mod read_video;
pub mod read_video_port;
pub use read_image::{ImageCodecPort, ReadImagePolicyPort, ReadImageRuntime};
pub use read_video::{ReadVideoHostPort, ReadVideoRuntime};
pub mod read_document_cache;
mod read_pdf_host;
pub mod read_pdf_native;
pub mod read_svg_native;
pub use read_pdf_host::{PdfScreenshot, PdfScreenshotPort};
pub mod read_summary;
pub mod resolve;
pub mod write;
mod write_archive;
mod write_sqlite;

use ara_agent::{AgentTool, ToolError, ToolOutput};
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use tokio_util::sync::CancellationToken;

/// Upstream `DEFAULT_MAX_LINES` / `DEFAULT_MAX_BYTES` (session/streaming-output.ts).
pub const DEFAULT_MAX_LINES: usize = 3000;
pub const DEFAULT_MAX_BYTES: usize = 50 * 1024;

/// A host's routing decision for a hierarchical content URI. Removed routes
/// remain distinct from unregistered ones: dropping an explicit override must
/// not silently restore a native handler (OMP `InternalUrlRouter.unregister`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentUriRoute {
    Unregistered,
    Registered { writable: bool },
    Removed,
}

/// UTF-8 content returned by a host-owned URI handler. It has no backing file;
/// read selectors operate on this content without materializing a local path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UriResource {
    pub content: String,
    pub content_type: String,
    pub notes: Vec<String>,
    pub immutable: bool,
}

/// An internal Host's authorized file. Remote URI responses cannot
/// supply this capability; ports opt in through `read_file` below.
#[derive(Clone, Debug)]
pub struct UriFileResource {
    pub path: PathBuf,
    pub content_type: String,
    pub max_inline_bytes: Option<u64>,
    /// Immutable artifacts bypass mutable hashline and EditStore consumers.
    pub immutable: bool,
}

/// The same distinctions as the Host router's SchemeSpec.write.via/scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentUriEditPolicy {
    ReadOnly,
    Handler,
    File { plan_writable: bool },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentUriEditSpec {
    pub scheme: String,
    pub single_slash_alias: bool,
    pub line_selectors: bool,
    pub port_authority: bool,
    pub immutable: bool,
    pub policy: ContentUriEditPolicy,
}

pub mod edit_urls;

/// Fixed b91 WRITE handler policy. Transport/approval classification never grants filesystem access.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ContentUriWritePayload { #[default] Normalized, Verbatim }
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ContentUriWriteScope { #[default] Content, Coordination }
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ContentUriWriteTier { #[default] Write, Read }
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ContentUriWritePolicy {
    pub payload: ContentUriWritePayload,
    pub scope: ContentUriWriteScope,
    pub tier: ContentUriWriteTier,
}
impl ContentUriWritePolicy {
    pub fn coordination(self) -> bool { self.scope == ContentUriWriteScope::Coordination }
}

#[derive(Debug)]
pub enum ContentUriWriteDispatchError {NotDispatched(ToolError),Effect(ToolError)}
impl From<ToolError> for ContentUriWriteDispatchError {fn from(error:ToolError)->Self{Self::Effect(error)}}
impl std::fmt::Display for ContentUriWriteDispatchError {fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{match self{Self::NotDispatched(error)|Self::Effect(error)=>f.write_str(&error.0)}}}
impl std::error::Error for ContentUriWriteDispatchError {}

/// A private Host observation, never a permission supplied by model arguments.
/// Wrappers forward the selected leaf stamp. Mutable leaves advance epoch at
/// every publication, including ABA replacement and explicit removal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentUriWriteSelection {
    pub owner: usize,
    pub epoch: u64,
    pub route: ContentUriRoute,
    pub policy: ContentUriWritePolicy,
}
#[derive(Clone)]
pub struct ContentUriWriteReceipt {
    context:ToolContext,
    pub url: String,
    pub port: Option<Arc<dyn ContentUriPort>>,
    pub selection: Option<ContentUriWriteSelection>,
}
impl ContentUriWriteReceipt {
    pub fn capture(context: &ToolContext, raw_path: &str) -> Self {
        let path=ara_edit::path_policy::unwrap_hashline_header_path(raw_path);
        let (url,_)=crate::internal_urls::split_content_url_selector(path);
        let port=context.content_uri_port();
        let selection=port.as_ref().map(|port|port.write_selection(&url));
        Self { context:context.clone(),url, port, selection }
    }
    pub fn check_current(&self)->Result<(),ToolError>{self.check(&self.context,&self.url)}
    pub fn check(&self, context:&ToolContext, raw_path:&str)->Result<(),ToolError> {
        let now=Self::capture(context,raw_path);
        let same_owner=match (&self.port,&now.port) {
            (None,None)=>true,(Some(a),Some(b))=>Arc::ptr_eq(a,b),_=>false,
        };
        if self.url!=now.url||!same_owner||self.selection!=now.selection {
            return Err(ToolError("URI selected path/leaf/epoch changed after approval; not dispatched; fresh approval required".into()));
        }
        Ok(())
    }
}

/// Optional per-host content routing for `read` and `write`. The host owns
/// registrations, permissions and request transport; ordinary tool gates still
/// run before these methods. Other tools retain their filesystem contracts.
#[async_trait]
pub trait ContentUriPort: Send + Sync {
    /// Actual Host scheme declarations for edit. A write handler is not a
    /// mutable file grant. Defaults intentionally grant no file mutation.
    fn edit_specs(&self) -> Vec<ContentUriEditSpec> {
        Vec::new()
    }
    fn edit_sandbox_roots(&self) -> Vec<(String, PathBuf)> {
        Vec::new()
    }
    /// Resolve a file-written mutable scheme with create semantics. Hosts must
    /// validate containment for missing targets as well as existing symlinks.
    /// The caller checks the actual spec and re-asks before apply.
    async fn resolve_edit_file(
        &self,
        _url: &str,
        _cancel: CancellationToken,
    ) -> Result<Option<UriFileResource>, ToolError> {
        Ok(None)
    }
    fn route(&self, url: &str) -> ContentUriRoute;
    /// Conservative default; dispatch and approval query the same selected owner.
    fn write_policy(&self, _url: &str) -> ContentUriWritePolicy { ContentUriWritePolicy::default() }
    fn write_selection(&self,url:&str)->ContentUriWriteSelection {
        ContentUriWriteSelection { owner:self as *const Self as *const () as usize,
            epoch:0,route:self.route(url),policy:self.write_policy(url) }
    }
    /// Immutable/default handlers check synchronously immediately before entry.
    /// Mutable transports override this and check at their effect admission lock.
    async fn write_result_selected(&self,url:&str,content:&str,selection:&ContentUriWriteSelection,cancel:CancellationToken)->Result<Option<ToolOutput>,ContentUriWriteDispatchError> {
        if &self.write_selection(url)!=selection {
            return Err(ContentUriWriteDispatchError::NotDispatched(ToolError("Selected URI handler changed before dispatch; nothing sent".into())));
        }
        self.write_result(url,content,cancel).await.map_err(Into::into)
    }
    /// Pure registered-alias normalization; preserves scheme case, never performs IO.
    fn normalize_url(&self, input: &str) -> String {
        input.to_owned()
    }
    /// Host-authorized linkable synchronous backing, not an async read bridge.
    fn locate_sync(&self, _url: &str) -> Result<Option<PathBuf>, ToolError> {
        Ok(None)
    }
    /// Native artifacts can be larger than their inline limit. Let the read
    /// tool apply its existing bounded file window without loading the blob.
    async fn read_file(
        &self,
        _url: &str,
        _cancel: CancellationToken,
    ) -> Result<Option<UriFileResource>, ToolError> {
        Ok(None)
    }
    /// Search/edit needs a backing path without materializing large content.
    /// Hosts may return a directory; the default retains existing file ports.
    async fn search_path_only(
        &self,
        url: &str,
        cancel: CancellationToken,
    ) -> Result<Option<PathBuf>, ToolError> {
        Ok(self.read_file(url, cancel).await?.map(|file| file.path))
    }
    async fn read(&self, url: &str, cancel: CancellationToken) -> Result<UriResource, ToolError>;
    async fn write(
        &self,
        url: &str,
        content: &str,
        cancel: CancellationToken,
    ) -> Result<(), ToolError>;
    /// A Host device may return its own tool receipt (for example plan approval).
    /// The default performs exactly one existing write and keeps its usual receipt.
    async fn write_result(
        &self,
        url: &str,
        content: &str,
        cancel: CancellationToken,
    ) -> Result<Option<ToolOutput>, ToolError> {
        self.write(url, content, cancel).await?;
        Ok(None)
    }
}

/// Fixed OMP ToolSession device-only transport flags, owned by the current Host.
/// A description preview is independent of the actual file grant.
#[derive(Clone, Copy, Debug, Default)]
pub struct WriteTransportPolicy {
    pub device_only: bool,
    pub pending_full_description: bool,
}

/// Shared tool settings chosen by the host.
#[derive(Clone)]
pub struct ToolContext {
    pub cwd: PathBuf,
    /// `N|text` line prefixes on reads (OMP `readLineNumbers`, default off).
    pub line_numbers: bool,
    /// Host snapshot of fixed tools.outputMaxColumns; UTF-16 units, 0 disables.
    read_output_max_columns: f64,
    /// Edit mode of the `edit` tool (OMP `edit.mode`, default hashline).
    edit_mode: ara_edit::EditMode,
    /// Native file-display resolution can differ from a captured tool override.
    display_edit_mode: ara_edit::EditMode,
    /// Whether the host exposes the `edit` tool (OMP `hasEditTool`, default true).
    edit_enabled: bool,
    edit_settings: edit::EditSettings,
    edit_runtime_policy: Option<Arc<dyn edit_runtime_policy::EditRuntimePolicy>>,
    grammar_host: Arc<RwLock<Option<Arc<dyn grammars::GrammarHostPort>>>>,
    edit_host: Option<Arc<dyn edit_host::EditHostPort>>,
    edit_post_apply: Arc<RwLock<Option<Arc<dyn edit_post_apply::EditPostApplyHost>>>>,
    /// File snapshots shared by `read`, `grep` and `edit` for one session
    /// (OMP `getEditStore(session)`): tags, seen lines, clipboard registers.
    pub edit_store: ara_edit::EditStore,
    /// Pending proposal registry shared by Session tool snapshots and writes.
    pub pending_resolution: resolve::PendingResolutionQueue,
    /// Registered merge regions belong to this logical Session; clones share them.
    pub conflict_history: Arc<std::sync::Mutex<conflict_detect::ConflictHistory>>,
    /// Skills `skill://` URLs resolve against (the host's loaded skills).
    pub skills: std::sync::Arc<std::sync::RwLock<Vec<internal_urls::SkillRef>>>,
    content_uri_port: Arc<RwLock<Option<Arc<dyn ContentUriPort>>>>,
    write_transport: Arc<RwLock<WriteTransportPolicy>>,
    read_text_metrics: Option<Arc<dyn read_sqlite::ReadTextMetrics>>,
    read_remote_mount_root: Option<PathBuf>,
    /// Snapshot retains the issuing Session owner; its reference remains live.
    read_plan_reference_port: Option<Arc<dyn read::ReadPlanReferencePort>>,
    archive_date_port: Option<Arc<dyn ArchiveDatePort>>,
    pdf_adapter: Arc<dyn read_document::PdfNativeAdapter>,
    document_cache: Option<Arc<read_document_cache::DocumentCache>>,
    pdf_screenshot_port: Option<Arc<dyn PdfScreenshotPort>>,
    read_image_codec: Option<Arc<dyn ImageCodecPort>>,
    read_image_policy: Option<Arc<dyn ReadImagePolicyPort>>,
    read_video_runtime: Option<Arc<ReadVideoRuntime>>,
    read_auto_resize_images: bool,
    read_render_markdown: bool,
    read_summary_policy: read_summary::SummaryPolicy,
    /// Fixed read.defaultLimit raw Host number, clamped by ReadTool consumers.
    read_default_limit: f64,
    /// Logical Session memo shared by immutable tool snapshots, never by new Sessions.
    read_summary_cache: Arc<read_summary::SummaryCache>,
}

impl std::fmt::Debug for ToolContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolContext")
            .field("cwd", &self.cwd)
            .field("line_numbers", &self.line_numbers)
            .field("edit_mode", &self.edit_mode)
            .field("display_edit_mode", &self.display_edit_mode)
            .field("edit_enabled", &self.edit_enabled)
            .finish_non_exhaustive()
    }
}

impl ToolContext {
    /// A relative `cwd` is made absolute against the process directory, so
    /// `.` names a real directory rather than an empty path.
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        let cwd = cwd.into();
        let cwd = std::path::absolute(&cwd).unwrap_or(cwd);
        ToolContext {
            cwd: normalize(&cwd),
            line_numbers: false,
            read_output_max_columns: 768.0,
            edit_mode: ara_edit::EditMode::Hashline,
            display_edit_mode: ara_edit::EditMode::Hashline,
            edit_enabled: true,
            edit_settings: Default::default(),
            edit_runtime_policy: None,
            grammar_host: Default::default(),
            edit_host: None,
            edit_post_apply: Default::default(),
            edit_store: ara_edit::EditStore::new(),
            pending_resolution: Default::default(),
            conflict_history: Default::default(),
            skills: Default::default(),
            content_uri_port: Default::default(),
            write_transport: Default::default(),
            read_text_metrics: None,
            read_remote_mount_root: None,
            read_plan_reference_port: None,
            archive_date_port: None,
            pdf_adapter: Arc::new(read_document::NativePdfAdapter),
            document_cache: None,
            pdf_screenshot_port: None,
            read_image_codec: None,
            read_image_policy: None,
            read_video_runtime: None,
            read_auto_resize_images: true,
            read_render_markdown: false,
            read_summary_policy: Default::default(),
            read_default_limit: 300.0,
            read_summary_cache: Default::default(),
        }
    }

    /// Host-owned optional document conversion cache; tools never choose a root.
    pub fn with_document_cache(mut self, cache: Arc<read_document_cache::DocumentCache>) -> Self {
        self.document_cache = Some(cache);
        self
    }
    pub fn with_pdf_adapter(mut self, adapter: Arc<dyn read_document::PdfNativeAdapter>) -> Self {
        self.pdf_adapter = adapter;
        self
    }
    pub fn with_pdf_screenshot_port(mut self, port: Arc<dyn PdfScreenshotPort>) -> Self {
        self.pdf_screenshot_port = Some(port);
        self
    }
    pub(crate) fn pdf_adapter(&self) -> Arc<dyn read_document::PdfNativeAdapter> {
        self.pdf_adapter.clone()
    }
    pub(crate) fn document_cache(&self) -> Option<Arc<read_document_cache::DocumentCache>> {
        self.document_cache.clone()
    }
    pub(crate) fn pdf_screenshot_port(&self) -> Option<Arc<dyn PdfScreenshotPort>> {
        self.pdf_screenshot_port.clone()
    }

    /// Share the Host process media runtime, including its fixed version cache.
    pub fn with_read_video_runtime(mut self, runtime: Arc<ReadVideoRuntime>) -> Self {
        self.read_video_runtime = Some(runtime);
        self
    }
    pub(crate) fn read_video_runtime(&self) -> Option<Arc<ReadVideoRuntime>> {
        self.read_video_runtime.clone()
    }

    /// The codec only decodes/encodes; policy and ladders remain in Core.
    pub fn with_read_image_codec(mut self, codec: Arc<dyn ImageCodecPort>) -> Self {
        self.read_image_codec = Some(codec);
        self
    }
    /// Live Host availability/model/env owner, queried independently per read.
    pub fn with_read_image_policy(mut self, policy: Arc<dyn ReadImagePolicyPort>) -> Self {
        self.read_image_policy = Some(policy);
        self
    }
    /// Fixed images.autoResize is the immutable setting of this tool snapshot.
    pub fn with_read_auto_resize_images(mut self, enabled: bool) -> Self {
        self.read_auto_resize_images = enabled;
        self
    }
    pub fn read_auto_resize_images(&self) -> bool {
        self.read_auto_resize_images
    }
    pub(crate) fn read_image_codec(&self) -> Option<Arc<dyn ImageCodecPort>> {
        self.read_image_codec.clone()
    }
    pub(crate) fn read_image_policy(&self) -> Option<Arc<dyn ReadImagePolicyPort>> {
        self.read_image_policy.clone()
    }
    pub fn read_image_runtime(&self) -> ReadImageRuntime {
        self.read_image_policy
            .as_ref()
            .map(|policy| policy.current_state())
            .unwrap_or_default()
    }

    /// Install the Host's local calendar owner for ZIP/CAB/RAR archive dates.
    pub fn with_archive_date_port(mut self, port: Arc<dyn ArchiveDatePort>) -> Self {
        self.archive_date_port = Some(port);
        self
    }
    pub(crate) fn archive_date_ms(
        &self,
        year: i32,
        month: u32,
        day: u32,
        hour: u32,
        minute: u32,
        second: u32,
    ) -> Result<f64, ToolError> {
        self.archive_date_port
            .as_ref()
            .ok_or_else(|| ToolError("Host archive local Date is not installed".into()))?
            .to_utc_ms(ArchiveLocalDate {
                year,
                month,
                day,
                hour,
                minute,
                second,
            })
    }

    /// Fixed OMP read.renderMarkdown is opt-in; internal protocols own their metadata.
    pub fn with_read_render_markdown(mut self, enabled: bool) -> Self {
        self.read_render_markdown = enabled;
        self
    }
    pub fn read_render_markdown(&self) -> bool {
        self.read_render_markdown
    }

    pub fn with_read_output_max_columns(mut self, columns: f64) -> Self {
        self.read_output_max_columns = columns;
        self
    }
    pub fn read_output_max_columns(&self) -> f64 {
        self.read_output_max_columns
    }

    /// Host-prepared READ settings snapshot; clones already admitted keep theirs.
    /// The same logical Session's pure parse memo survives settings rebinding.
    pub fn with_read_summary_policy(mut self, policy: read_summary::SummaryPolicy) -> Self {
        self.read_summary_policy = policy;
        self
    }
    pub fn read_summary_policy(&self) -> read_summary::SummaryPolicy {
        self.read_summary_policy
    }
    pub fn with_read_default_limit(mut self, limit: f64) -> Self {
        self.read_default_limit = limit;
        self
    }
    pub fn read_default_limit(&self) -> f64 {
        // Fixed Math.max/Math.min propagate NaN; f64::max/min do not.
        if self.read_default_limit.is_nan() {
            f64::NAN
        } else {
            self.read_default_limit
                .clamp(1.0, crate::DEFAULT_MAX_LINES as f64)
        }
    }
    pub(crate) fn read_summary_cache(&self) -> Arc<read_summary::SummaryCache> {
        self.read_summary_cache.clone()
    }

    /// Set the skills `skill://` URLs resolve against. The list is this
    /// context's own: clones made earlier keep theirs.
    pub fn with_skills(mut self, skills: Vec<internal_urls::SkillRef>) -> Self {
        self.skills = std::sync::Arc::new(std::sync::RwLock::new(skills));
        self
    }

    /// Install the content URI port shared by this context's existing clones.
    pub fn with_uri_port(self, port: Arc<dyn ContentUriPort>) -> Self {
        self.set_uri_port(port);
        self
    }

    /// Prepare a new immutable tool snapshot without altering existing clones.
    /// Its newly constructed tools share this new slot; publication belongs to
    /// the Host. The existing with/set_uri_port retain their shared semantics.
    pub fn with_isolated_uri_port(mut self, port: Option<Arc<dyn ContentUriPort>>) -> Self {
        self.content_uri_port = Arc::new(RwLock::new(port));
        self
    }

    pub fn write_transport_policy(&self) -> WriteTransportPolicy {
        *self
            .write_transport
            .read()
            .unwrap_or_else(|e| e.into_inner())
    }
    pub fn set_device_only_write(&self, enabled: bool) {
        self.write_transport
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .device_only = enabled;
    }
    pub fn set_pending_full_write_description(&self, enabled: bool) {
        self.write_transport
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .pending_full_description = enabled;
    }

    pub fn set_uri_port(&self, port: Arc<dyn ContentUriPort>) {
        *self
            .content_uri_port
            .write()
            .unwrap_or_else(|e| e.into_inner()) = Some(port);
    }

    /// Clone the port while holding the slot lock; release it before awaiting
    /// the host so live registrations do not block behind a pending read/write.
    pub fn content_uri_port(&self) -> Option<Arc<dyn ContentUriPort>> {
        self.content_uri_port
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Immutable Host snapshot of the existing Unicode/ANSI renderer service.
    pub fn with_read_text_metrics(
        mut self,
        metrics: Arc<dyn read_sqlite::ReadTextMetrics>,
    ) -> Self {
        self.read_text_metrics = Some(metrics);
        self
    }
    pub fn read_text_metrics(&self) -> Option<Arc<dyn read_sqlite::ReadTextMetrics>> {
        self.read_text_metrics.clone()
    }
    /// Install the same Session owner as EDIT. Earlier snapshots retain theirs.
    pub fn with_read_plan_reference_port(
        mut self,
        port: Arc<dyn read::ReadPlanReferencePort>,
    ) -> Self {
        self.read_plan_reference_port = Some(port);
        self
    }
    pub fn read_plan_reference_port(&self) -> Option<Arc<dyn read::ReadPlanReferencePort>> {
        self.read_plan_reference_port.clone()
    }
    /// Host data-root adaptation of fixed getRemoteDir; no mount is created.
    pub fn with_read_remote_mount_root(mut self, root: PathBuf) -> Self {
        self.read_remote_mount_root = Some(root);
        self
    }
    pub fn read_remote_mount_root(&self) -> Option<&Path> {
        self.read_remote_mount_root.as_deref()
    }

    /// Resolve an internal URL (`skill://…`) to a filesystem path.
    pub fn resolve_internal_url(&self, url: &str) -> Result<PathBuf, String> {
        let skills = self.skills.read().unwrap_or_else(|e| e.into_inner());
        internal_urls::resolve_skill_url(&skills, url)
    }

    /// Resolve a skill URL to its backing path for `grep` and `glob`.
    pub fn resolve_internal_url_path_only(&self, url: &str) -> Result<PathBuf, String> {
        let skills = self.skills.read().unwrap_or_else(|e| e.into_inner());
        internal_urls::resolve_skill_url_path_only(&skills, url)
    }

    /// Select the edit mode and whether the `edit` tool is exposed.
    pub fn with_edit(self, mode: ara_edit::EditMode, edit_enabled: bool) -> Self {
        self.with_edit_modes(mode, mode, edit_enabled)
    }

    /// Prepare an immutable model-dependent snapshot on a clone. EditStore,
    /// Skill references and the URI port retain their existing shared owners;
    /// already admitted tool clones keep their previous tool/display modes.
    pub fn with_edit_modes(
        mut self,
        tool_mode: ara_edit::EditMode,
        display_mode: ara_edit::EditMode,
        edit_enabled: bool,
    ) -> Self {
        self.edit_mode = tool_mode;
        self.display_edit_mode = display_mode;
        self.edit_enabled = edit_enabled;
        self
    }

    /// Host-prepared settings snapshot; existing clones retain their policy.
    pub fn with_edit_settings(mut self, settings: edit::EditSettings) -> Self {
        self.edit_settings = settings;
        self
    }

    pub fn with_edit_runtime_policy(
        mut self,
        policy: Arc<dyn edit_runtime_policy::EditRuntimePolicy>,
    ) -> Self {
        self.edit_runtime_policy = Some(policy);
        self
    }
    pub fn edit_runtime_reader_identity(&self) -> Option<String> {
        self.edit_runtime_policy
            .as_ref()
            .and_then(|policy| policy.reader_identity())
    }
    pub fn set_grammar_host(&self, host: Arc<dyn grammars::GrammarHostPort>) {
        *self
            .grammar_host
            .write()
            .unwrap_or_else(|error| error.into_inner()) = Some(host);
    }
    pub fn grammar_host(&self) -> Option<Arc<dyn grammars::GrammarHostPort>> {
        self.grammar_host
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }
    /// Freeze one successful live read for a single READ/EDIT action. Cloning
    /// this context preserves all Session/URI/Store owners; no fallback remains.
    pub fn with_edit_runtime_snapshot(
        mut self,
        snapshot: edit_runtime_policy::EditRuntimeSnapshot,
    ) -> Self {
        self.edit_mode = snapshot.mode;
        self.display_edit_mode = snapshot.display_mode;
        self.edit_settings = snapshot.settings;
        self.edit_runtime_policy = None;
        self
    }
    pub fn try_edit_runtime_policy(
        &self,
    ) -> Result<edit_runtime_policy::EditRuntimeSnapshot, ToolError> {
        match &self.edit_runtime_policy {
            Some(policy) => policy.snapshot(),
            None => Ok(self.edit_runtime_fallback()),
        }
    }
    fn edit_runtime_fallback(&self) -> edit_runtime_policy::EditRuntimeSnapshot {
        edit_runtime_policy::EditRuntimeSnapshot {
            mode: self.edit_mode,
            display_mode: self.display_edit_mode,
            settings: self.edit_settings,
            compact_prompt: false,
        }
    }
    pub fn edit_settings(&self) -> edit::EditSettings {
        self.try_edit_runtime_policy()
            .unwrap_or_else(|_| self.edit_runtime_fallback())
            .settings
    }

    /// Attach the Host's Session facade. Tool snapshots retain this capability;
    /// the Host supplies current plan/artifact policy when an edit starts.
    /// Install LSP/deferred Session services before EditTool/kernel creation:
    /// their callbacks and settings are captured by the constructor.
    pub fn with_edit_host(mut self, host: Arc<dyn edit_host::EditHostPort>) -> Self {
        self.edit_host = Some(host);
        self
    }

    pub fn edit_host(&self) -> Option<Arc<dyn edit_host::EditHostPort>> {
        self.edit_host.clone()
    }

    /// Bind the existing Host Session after its journal/model owner is ready.
    /// An admitted edit captures the Arc once, before awaiting its write.
    pub fn set_edit_post_apply_host(&self, host: Arc<dyn edit_post_apply::EditPostApplyHost>) {
        *self
            .edit_post_apply
            .write()
            .unwrap_or_else(|e| e.into_inner()) = Some(host);
    }

    pub fn edit_post_apply_host(&self) -> Option<Arc<dyn edit_post_apply::EditPostApplyHost>> {
        self.edit_post_apply
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn edit_mode(&self) -> ara_edit::EditMode {
        self.try_edit_runtime_policy()
            .unwrap_or_else(|_| self.edit_runtime_fallback())
            .mode
    }

    pub fn display_edit_mode(&self) -> ara_edit::EditMode {
        self.try_edit_runtime_policy()
            .unwrap_or_else(|_| self.edit_runtime_fallback())
            .display_mode
    }

    /// Hashline display for `read`, `grep` and `write`: `[path#TAG]` headers
    /// and `N:text` rows (OMP `resolveFileDisplayMode`: the edit tool is
    /// exposed in hashline mode).
    pub fn hashlines(&self) -> bool {
        self.edit_enabled && self.display_edit_mode() == ara_edit::EditMode::Hashline
    }

    /// Header path for a hashline read (`formatReadHashlineHeader`): cwd-relative
    /// inside cwd, otherwise absolute with the home directory shortened to `~`.
    pub fn hashline_display(&self, abs: &Path) -> String {
        let display = paths::format_path_relative_to_cwd(abs, &self.cwd, false);
        if !Path::new(&display).is_absolute() {
            return display;
        }
        match std::env::var_os("HOME").map(PathBuf::from) {
            Some(home) if !home.as_os_str().is_empty() => match abs.strip_prefix(&home) {
                Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
                Ok(rest) => format!("~/{}", rest.to_string_lossy()),
                Err(_) => display,
            },
            _ => display,
        }
    }

    /// Resolve a model-supplied path: `~` expands to `$HOME`, relative paths
    /// join the working directory, and `.`/`..` are normalized lexically
    /// (OMP `resolveToCwd`/`expandPath`).
    pub fn resolve(&self, path: &str) -> PathBuf {
        let expanded = if path == "~" {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(path))
        } else if let Some(rest) = path.strip_prefix("~/") {
            std::env::var_os("HOME")
                .map(|h| PathBuf::from(h).join(rest))
                .unwrap_or_else(|| PathBuf::from(path))
        } else {
            PathBuf::from(path)
        };
        let joined = if expanded.is_absolute() {
            expanded
        } else {
            self.cwd.join(expanded)
        };
        normalize(&joined)
    }

    /// Path shown back to the model: relative to cwd when inside it.
    pub fn display(&self, path: &Path) -> String {
        path.strip_prefix(&self.cwd)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| path.to_string_lossy().into_owned())
    }
}

/// Lexical normalization of `.` and `..` (no filesystem access, no symlink resolution).
pub fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `read`, `write`, `edit`, `bash`, `grep` and `glob` bound to one working
/// directory and one snapshot store.
pub fn builtin_tools(ctx: ToolContext) -> Vec<Arc<dyn AgentTool>> {
    vec![
        Arc::new(read::ReadTool { ctx: ctx.clone() }),
        Arc::new(write::WriteTool::new(ctx.clone())),
        Arc::new(edit::EditTool::new(ctx.clone())),
        Arc::new(bash::BashTool { ctx: ctx.clone() }),
        Arc::new(grep::GrepTool::new(ctx.clone())),
        Arc::new(glob::GlobTool::new(ctx)),
    ]
}

/// Run blocking tool work on the blocking pool. The work gets a child of
/// `cancel`; aborting the run, or dropping the returned future (a host-side
/// timeout), cancels it, so abandoned filesystem scans stop promptly.
pub(crate) async fn run_blocking<F>(
    label: &'static str,
    cancel: tokio_util::sync::CancellationToken,
    f: F,
) -> Result<ara_agent::ToolOutput, ara_agent::ToolError>
where
    F: FnOnce(
            tokio_util::sync::CancellationToken,
        ) -> Result<ara_agent::ToolOutput, ara_agent::ToolError>
        + Send
        + 'static,
{
    let work = cancel.child_token();
    let _stop_on_drop = work.clone().drop_guard();
    let job = tokio::task::spawn_blocking(move || f(work));
    tokio::select! {
        r = job => r.map_err(|e| ara_agent::ToolError(format!("{label} failed: {e}")))?,
        _ = cancel.cancelled() => Err(ara_agent::ToolError(format!("{label} was aborted"))),
    }
}

/// Human-readable byte size (OMP `formatBytes`).
pub fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1}GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}
