//! Reference Host artifact storage from fixed OMP 596f2da (MIT).
//! Source: coding-agent/session/{artifacts,session-manager}.ts and
//! internal-urls/artifact-protocol.ts. Data locations and URI authority are Host-owned.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use anyhow::{Context, Result, bail};
use ara_agent::ToolError;
use ara_session::SessionJournal;
use ara_tools::{ContentUriPort, ContentUriRoute, UriFileResource, UriResource};
use async_trait::async_trait;
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

const MAX_INLINE_ARTIFACT_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Default)]
struct ArtifactState {
    next_id: Option<u64>,
    memory: HashMap<String, String>,
}

pub struct SessionArtifacts {
    directory: Option<PathBuf>,
    state: Mutex<ArtifactState>,
}

impl SessionArtifacts {
    pub fn for_journal(journal: &SessionJournal) -> Arc<Self> {
        let directory = (journal.is_persistent() && journal.path().extension().is_some_and(|ext| ext == "jsonl"))
            .then(|| journal.path().with_extension(""));
        Arc::new(Self { directory, state: Mutex::new(ArtifactState::default()) })
    }

    pub fn directory(&self) -> Option<&Path> {
        self.directory.as_deref()
    }

    pub async fn save(&self, content: &str, tool_type: &str) -> Result<String> {
        // This lock shares first-use initialization and allocation, including
        // the initial directory scan, across concurrent callers.
        let mut state = self.state.lock().await;
        if state.next_id.is_none() {
            let mut next = 0u64;
            if let Some(directory) = &self.directory {
                tokio::fs::create_dir_all(directory).await.context("creating Session artifact directory")?;
                let mut files = tokio::fs::read_dir(directory).await.context("scanning Session artifact IDs")?;
                while let Some(file) = files.next_entry().await? {
                    let name = file.file_name().to_string_lossy().into_owned();
                    if name.ends_with(".log")
                        && let Some((prefix, _)) = name.split_once('.')
                        && !prefix.is_empty()
                        && prefix.bytes().all(|byte| byte.is_ascii_digit())
                        && let Ok(id) = prefix.parse::<u64>()
                    {
                        next = next.max(id.checked_add(1).context("artifact ID space exhausted")?);
                    }
                }
            }
            state.next_id = Some(next);
        }
        let id = state.next_id.expect("initialized artifact counter");
        state.next_id = Some(id.checked_add(1).context("artifact ID space exhausted")?);
        let id = id.to_string();
        if let Some(directory) = &self.directory {
            let destination = directory.join(format!("{id}.{}.log", sanitize_tool_type(tool_type)));
            write_artifact(&destination, content).await?;
        } else {
            state.memory.insert(id.clone(), content.to_owned());
        }
        Ok(id)
    }

    async fn find_path(&self, id: &str) -> Result<PathBuf> {
        let directory = self.directory.as_ref().context("No session - artifacts unavailable")?;
        let mut files = match tokio::fs::read_dir(directory).await {
            Ok(files) => files,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => bail!("No artifacts directory found"),
            Err(error) => return Err(error).context("reading Session artifacts"),
        };
        let mut available = Vec::new();
        while let Some(file) = files.next_entry().await? {
            let name = file.file_name().to_string_lossy().into_owned();
            if name.starts_with(&format!("{id}.")) {
                let meta = file.metadata().await?;
                if meta.is_dir() {
                    bail!("Artifact {id} resolved to a directory, not a file");
                }
                return Ok(file.path());
            }
            if let Some((prefix, _)) = name.split_once('.')
                && !prefix.is_empty()
                && prefix.bytes().all(|byte| byte.is_ascii_digit())
            {
                available.push(prefix.to_owned());
            }
        }
        available.sort_by(|left, right| left.len().cmp(&right.len()).then_with(|| left.cmp(right)));
        available.dedup();
        bail!(
            "Artifact {id} not found. Available: {}",
            if available.is_empty() { "none".into() } else { available.join(", ") }
        )
    }
}

fn sanitize_tool_type(tool_type: &str) -> String {
    let mut sanitized = String::new();
    let mut replaced = false;
    for character in tool_type.chars() {
        if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
            sanitized.push(character);
            replaced = false;
        } else if !replaced {
            sanitized.push('_');
            replaced = true;
        }
    }
    sanitized.truncate(sanitized.len().min(64));
    let sanitized = sanitized.trim_matches('_');
    if sanitized.is_empty() { "tool".into() } else { sanitized.into() }
}

/// Verify the staged bytes and readability before publishing a completed blob.
async fn write_artifact(destination: &Path, content: &str) -> Result<()> {
    let temporary = PathBuf::from(format!("{}.tmp-{}", destination.display(), uuid::Uuid::new_v4()));
    let result = async {
        tokio::fs::write(&temporary, content.as_bytes()).await.context("writing staged artifact")?;
        let mut file = tokio::fs::File::open(&temporary).await.context("checking staged artifact readability")?;
        let size = file.metadata().await?.len();
        if size != content.len() as u64 {
            bail!("Artifact size mismatch: found {size} of {} bytes", content.len());
        }
        if size > 0 {
            let mut first = [0u8; 1];
            file.read_exact(&mut first).await.context("reading staged artifact")?;
        }
        drop(file);
        tokio::fs::rename(&temporary, destination).await.context("publishing artifact")?;
        Ok(())
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&temporary).await;
    }
    result
}

/// Explicit RPC registrations and tombstones take precedence over the native
/// handler. The selected store is captured before each asynchronous read.
pub struct ArtifactUriRouter {
    current: RwLock<Arc<SessionArtifacts>>,
    fallback: Option<Arc<dyn ContentUriPort>>,
}

impl ArtifactUriRouter {
    pub fn new(store: Arc<SessionArtifacts>, fallback: Option<Arc<dyn ContentUriPort>>) -> Arc<Self> {
        Arc::new(Self { current: RwLock::new(store), fallback })
    }
    pub fn bind(&self, store: Arc<SessionArtifacts>) {
        *self.current.write().unwrap_or_else(|error| error.into_inner()) = store;
    }
    fn native_route(&self, url: &str) -> bool {
        url.split_once("://").is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("artifact"))
            && self.fallback.as_ref().is_none_or(|port| port.route(url) == ContentUriRoute::Unregistered)
    }
    pub fn current_store(&self) -> Arc<SessionArtifacts> {
        self.current.read().unwrap_or_else(|error| error.into_inner()).clone()
    }
}

fn artifact_id(url: &str) -> Result<&str> {
    let (_, authority) = url.split_once("://").context("Invalid artifact URL")?;
    let id = authority.split(['/', '?', '#']).next().unwrap_or_default();
    if id.is_empty() {
        bail!("artifact:// URL requires a numeric ID: artifact://0");
    }
    if !id.bytes().all(|byte| byte.is_ascii_digit()) {
        bail!("artifact:// ID must be numeric, got: {id}");
    }
    Ok(id)
}

#[async_trait]
impl ContentUriPort for ArtifactUriRouter {
    fn route(&self, url: &str) -> ContentUriRoute {
        if let Some(port) = &self.fallback {
            let route = port.route(url);
            if route != ContentUriRoute::Unregistered {
                return route;
            }
        }
        if self.native_route(url) {
            ContentUriRoute::Registered { writable: false }
        } else {
            ContentUriRoute::Unregistered
        }
    }
    async fn read_file(&self, url: &str, cancel: CancellationToken) -> Result<Option<UriFileResource>, ToolError> {
        if !self.native_route(url) {
            return match &self.fallback {
                Some(port) => port.read_file(url, cancel).await,
                None => Ok(None),
            };
        }
        if cancel.is_cancelled() {
            return Err(ToolError("Artifact read aborted".into()));
        }
        let id = artifact_id(url).map_err(|error| ToolError(error.to_string()))?;
        let store = self.current_store();
        if store.directory.is_none() {
            return Err(ToolError("No session - artifacts unavailable".into()));
        }
        let path = store.find_path(id).await.map_err(|error| ToolError(error.to_string()))?;
        if cancel.is_cancelled() {
            return Err(ToolError("Artifact read aborted".into()));
        }
        Ok(Some(UriFileResource {
            path,
            content_type: "text/plain".into(),
            max_inline_bytes: Some(MAX_INLINE_ARTIFACT_BYTES),
        }))
    }
    async fn read(&self, url: &str, cancel: CancellationToken) -> Result<UriResource, ToolError> {
        if !self.native_route(url) {
            return self
                .fallback
                .as_ref()
                .ok_or_else(|| ToolError("Unknown artifact route".into()))?
                .read(url, cancel)
                .await;
        }
        if cancel.is_cancelled() {
            return Err(ToolError("Artifact read aborted".into()));
        }
        let id = artifact_id(url).map_err(|error| ToolError(error.to_string()))?;
        let store = self.current_store();
        let content = if store.directory.is_some() {
            let path = store.find_path(id).await.map_err(|error| ToolError(error.to_string()))?;
            let size = tokio::fs::metadata(&path).await.map_err(|error| ToolError(error.to_string()))?.len();
            if size > MAX_INLINE_ARTIFACT_BYTES {
                return Err(ToolError(format!(
                    "Artifact {id} is {size} bytes; full internal resolution is blocked. Use read selectors such as {url}:1-3000"
                )));
            }
            let bytes = tokio::fs::read(path).await.map_err(|error| ToolError(error.to_string()))?;
            String::from_utf8_lossy(&bytes).into_owned()
        } else {
            return Err(ToolError("No session - artifacts unavailable".into()));
        };
        if cancel.is_cancelled() {
            return Err(ToolError("Artifact read aborted".into()));
        }
        Ok(UriResource { content, content_type: "text/plain".into(), notes: Vec::new(), immutable: true })
    }
    async fn write(&self, url: &str, content: &str, cancel: CancellationToken) -> Result<(), ToolError> {
        if self.native_route(url) {
            return Err(ToolError("artifact:// resources are immutable".into()));
        }
        self.fallback
            .as_ref()
            .ok_or_else(|| ToolError("Unknown artifact route".into()))?
            .write(url, content, cancel)
            .await
    }
}
