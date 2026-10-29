//! Host-owned fixed OMP vault protocol, cached roots, and Obsidian process owner.
//! Port of packages/coding-agent/src/internal-urls/vault-protocol.ts at
//! 596f2da7101178214aa27a753529d15e6b7ad91d. No product state or credentials.
// Copyright (c) 2025 Mario Zechner
// Copyright (c) 2025-2026 Can Bölük
// Copyright (c) 2026 Stencil Labs, Inc.
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
// THE SOFTWARE.

use ara_agent::{ToolError, ToolOutput};
use ara_tools::{ContentUriPort, ContentUriRoute, UriFileResource, UriResource};
use async_trait::async_trait;
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command, sync::Mutex as AsyncMutex};
use tokio_util::sync::CancellationToken;

const DARWIN_BINARY: &str = "/Applications/Obsidian.app/Contents/MacOS/obsidian";
const TIMEOUT_MS: u64 = 30_000;
const DISABLED: &str =
    "vault:// is disabled. Enable it by setting `vault.enabled = true` (Settings → Tools → Obsidian Vault).";
const MISSING_BINARY: &str = "Obsidian CLI binary not found. Checked PATH entry 'obsidian' and /Applications/Obsidian.app/Contents/MacOS/obsidian. Install Obsidian from https://obsidian.md or add its CLI binary to PATH.";
const MISSING_ROOT: &str =
    "vault:// path resolution requires a cached vault root; read vault:// first or use the write tool";

fn error(message: impl ToString) -> ToolError {
    ToolError(message.to_string())
}
fn cancelled(cancel: &CancellationToken) -> Result<(), ToolError> {
    if cancel.is_cancelled() { Err(error("obsidian command cancelled")) } else { Ok(()) }
}
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Kind {
    List,
    Info,
    Directory,
    File,
    FileOp,
    VaultOp,
}
#[derive(Clone, Debug)]
struct VaultUrl {
    url: String,
    kind: Kind,
    name: String,
    active: bool,
    relative: String,
    op: Option<String>,
    params: BTreeMap<String, String>,
}
fn decode(input: &str, query: bool) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let hex = |v: Option<&u8>| v.and_then(|v| (*v as char).to_digit(16));
            match (hex(bytes.get(at + 1)), hex(bytes.get(at + 2))) {
                (Some(a), Some(b)) => {
                    out.push((a * 16 + b) as u8);
                    at += 3;
                    continue;
                }
                _ if !query => return None,
                _ => {}
            }
        }
        out.push(if query && bytes[at] == b'+' { b' ' } else { bytes[at] });
        at += 1;
    }
    if query { Some(String::from_utf8_lossy(&out).into_owned()) } else { String::from_utf8(out).ok() }
}
fn encode(input: &str) -> String {
    let mut out = String::new();
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            out.push(byte as char);
        } else {
            use std::fmt::Write as _;
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}
fn encode_host(input: &str) -> String {
    encode(input).replace("%2F", "/")
}
fn validate_relative(value: &str) -> Result<(), ToolError> {
    ara_tools::internal_urls::validate_relative_path(value).map_err(|e| error(e.replace("skill://", "vault://")))
}
fn parse(url: &str) -> Result<VaultUrl, ToolError> {
    let scheme =
        ara_tools::internal_urls::hierarchical_scheme(url).ok_or_else(|| error(format!("Invalid URL: {url}")))?;
    if !scheme.eq_ignore_ascii_case("vault") {
        return Err(error(format!("Invalid vault URL: {url}")));
    }
    let rest = &url[scheme.len() + 3..];
    let without_hash = rest.split('#').next().unwrap_or_default();
    let (address, query) = without_hash.split_once('?').unwrap_or((without_hash, ""));
    let (host, raw_path) =
        address.split_once('/').map_or((address, "".into()), |(host, path)| (host, format!("/{path}")));
    let host = decode(host, false).unwrap_or_else(|| host.to_owned());
    let mut params = BTreeMap::new();
    for part in query.split('&').filter(|p| !p.is_empty()) {
        let (key, value) = part.split_once('=').unwrap_or((part, ""));
        params.insert(decode(key, true).unwrap_or_default(), decode(value, true).unwrap_or_default());
    }
    let has_path = !raw_path.is_empty() && raw_path != "/";
    let directory = raw_path == "/" || raw_path.ends_with('/');
    let relative = if has_path {
        let decoded = decode(&raw_path[1..].replace('\\', "/"), false)
            .ok_or_else(|| error(format!("Invalid URL encoding in vault:// path: {url}")))?;
        validate_relative(&decoded)?;
        decoded.trim_end_matches('/').to_owned()
    } else {
        String::new()
    };
    let op = params.get("op").filter(|op| !op.is_empty()).cloned();
    let kind = if let Some(op) = &op {
        let allowed = if relative.is_empty() {
            [
                "search",
                "daily",
                "daily-path",
                "tags",
                "tag",
                "tasks",
                "orphans",
                "unresolved",
                "deadends",
                "bases",
                "bookmarks",
                "recents",
                "templates",
                "aliases",
                "properties",
                "property",
            ]
            .contains(&op.as_str())
        } else {
            ["outline", "backlinks", "links", "tags", "properties", "tasks", "wordcount", "history", "base"]
                .contains(&op.as_str())
        };
        if !allowed {
            return Err(error(format!(
                "Unsupported vault:// {} op: {op}",
                if relative.is_empty() { "vault" } else { "file" }
            )));
        }
        if relative.is_empty() { Kind::VaultOp } else { Kind::FileOp }
    } else if host.is_empty() && !has_path {
        Kind::List
    } else if !has_path && raw_path != "/" {
        Kind::Info
    } else if directory {
        Kind::Directory
    } else {
        Kind::File
    };
    let active = host.is_empty() || host == "_";
    Ok(VaultUrl {
        url: url.to_owned(),
        kind,
        name: if active { "_".into() } else { host },
        active,
        relative,
        op,
        params,
    })
}
impl VaultUrl {
    fn param(&self, name: &str) -> Option<&str> {
        self.params.get(name).filter(|s| !s.is_empty()).map(String::as_str)
    }
    fn required(&self, name: &str) -> Result<&str, ToolError> {
        self.param(name).ok_or_else(|| {
            error(format!("vault://{} requires '{name}' query parameter", self.op.as_deref().unwrap_or_default()))
        })
    }
    fn vault_args(&self) -> Vec<String> {
        if self.active { vec![] } else { vec![format!("vault={}", self.name)] }
    }
    fn link(&self, relative: &str, directory: bool) -> String {
        let path = relative.split('/').filter(|s| !s.is_empty()).map(encode).collect::<Vec<_>>().join("/");
        format!("vault://{}/{}{}", encode_host(&self.name), path, if directory && !path.is_empty() { "/" } else { "" })
    }
}
fn invocation(parsed: &VaultUrl) -> Result<(Vec<String>, &'static str), ToolError> {
    let op = parsed.op.as_deref().unwrap_or_default();
    let mut args = parsed.vault_args();
    let (items, kind): (Vec<String>, _) = if parsed.kind == Kind::FileOp {
        let path = format!("path={}", parsed.relative);
        match op {
            "outline" => (vec!["outline".into(), path, "format=md".into()], "text/markdown"),
            "backlinks" => (vec!["backlinks".into(), path, "counts".into(), "format=tsv".into()], "text/plain"),
            "links" | "wordcount" | "history" => (vec![op.into(), path], "text/plain"),
            "tags" => (vec!["tags".into(), path, "counts".into(), "format=json".into()], "application/json"),
            "properties" => (vec!["properties".into(), path, "format=yaml".into()], "text/markdown"),
            "tasks" => (vec!["tasks".into(), path, "verbose".into(), "format=json".into()], "application/json"),
            "base" => (
                vec!["base:query".into(), path, format!("view={}", parsed.required("view")?), "format=md".into()],
                "text/markdown",
            ),
            _ => return Err(error(format!("Unsupported vault:// file op: {op}"))),
        }
    } else {
        match op {
            "search" => {
                let mut values = vec!["search:context".into(), format!("query={}", parsed.required("q")?)];
                if let Some(path) = parsed.param("path") {
                    validate_relative(&path.replace('\\', "/"))?;
                    values.push(format!("path={path}"));
                }
                if let Some(limit) = parsed.param("limit") {
                    values.push(format!("limit={limit}"));
                }
                if parsed.params.contains_key("case") {
                    values.push("case".into());
                }
                values.push("format=json".into());
                (values, "application/json")
            }
            "daily" => (vec!["daily:read".into()], "text/markdown"),
            "daily-path" => (vec!["daily:path".into()], "text/plain"),
            "tags" => (vec!["tags".into(), "counts".into(), "format=json".into()], "application/json"),
            "tag" => (
                vec![
                    "tag".into(),
                    format!(
                        "name={}",
                        match parsed.param("name") {
                            Some(name) => name,
                            None => parsed.required("tag")?,
                        }
                    ),
                    "verbose".into(),
                ],
                "text/plain",
            ),
            "tasks" => {
                (vec!["tasks".into(), "todo".into(), "verbose".into(), "format=json".into()], "application/json")
            }
            "unresolved" => {
                (vec!["unresolved".into(), "counts".into(), "verbose".into(), "format=json".into()], "application/json")
            }
            "bookmarks" | "aliases" => (vec![op.into(), "verbose".into(), "format=json".into()], "application/json"),
            "properties" => (vec!["properties".into(), "counts".into(), "format=yaml".into()], "text/markdown"),
            "property" => {
                let name = parsed.required("name")?;
                let path = parsed.required("path")?;
                validate_relative(&path.replace('\\', "/"))?;
                (vec!["property:read".into(), format!("name={name}"), format!("path={path}")], "text/plain")
            }
            "orphans" | "deadends" | "bases" | "recents" | "templates" => (vec![op.into()], "text/plain"),
            _ => return Err(error(format!("Unsupported vault:// vault op: {op}"))),
        }
    };
    args.extend(items);
    Ok((args, kind))
}

#[derive(Default)]
struct Cache {
    binary: Option<Option<PathBuf>>,
    directory: Option<BTreeMap<String, PathBuf>>,
    directory_order: Vec<String>,
    active: Option<PathBuf>,
    info: BTreeMap<String, String>,
}
#[derive(Default)]
struct ProcessVault {
    cache: Mutex<Cache>,
    discovery: AsyncMutex<()>,
}
static PROCESS_VAULT: OnceLock<Arc<ProcessVault>> = OnceLock::new();

pub struct HostVault {
    enabled: bool,
    process: Arc<ProcessVault>,
}
impl HostVault {
    pub fn new(enabled: bool) -> Arc<Self> {
        Arc::new(Self { enabled, process: PROCESS_VAULT.get_or_init(|| Arc::new(ProcessVault::default())).clone() })
    }
    pub fn enabled(&self) -> bool {
        self.enabled
    }
    pub fn has_obsidian(&self) -> bool {
        self.enabled && self.binary().is_some()
    }
    pub fn cached_roots(&self) -> Option<Vec<(String, PathBuf)>> {
        if !self.enabled {
            return None;
        }
        let cache = lock(&self.process.cache);
        let mut roots = Vec::new();
        if let Some(root) = &cache.active {
            roots.push(("_".into(), root.clone()));
        }
        if let Some(directory) = &cache.directory {
            roots.extend(
                cache
                    .directory_order
                    .iter()
                    .filter_map(|name| directory.get(name).map(|root| (name.clone(), root.clone()))),
            );
        }
        Some(roots)
    }
    pub fn port(self: &Arc<Self>, fallback: Option<Arc<dyn ContentUriPort>>) -> Arc<dyn ContentUriPort> {
        compose_ports(self.clone(), fallback)
    }
    fn check_enabled(&self) -> Result<(), ToolError> {
        if self.enabled { Ok(()) } else { Err(error(DISABLED)) }
    }
    fn binary(&self) -> Option<PathBuf> {
        let mut cache = lock(&self.process.cache);
        cache.binary.get_or_insert_with(find_binary).clone()
    }
    async fn spawn(&self, args: &[String], label: &str, cancel: CancellationToken) -> Result<String, ToolError> {
        self.check_enabled()?;
        cancelled(&cancel)?;
        let binary = self.binary().ok_or_else(|| error(MISSING_BINARY))?;
        let mut child = Command::new(binary)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(error)?;
        let mut stdout = child.stdout.take().expect("piped Obsidian stdout");
        let mut stderr = child.stderr.take().expect("piped Obsidian stderr");
        let mut out = tokio::spawn(async move {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).await.map(|_| bytes)
        });
        let mut err = tokio::spawn(async move {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).await.map(|_| bytes)
        });
        let stop = tokio::select! {
            biased;
            _ = cancel.cancelled() => Err("obsidian command cancelled".to_owned()),
            _ = tokio::time::sleep(Duration::from_millis(TIMEOUT_MS)) => Err(format!("obsidian command timed out after {TIMEOUT_MS}ms")),
            status = child.wait() => status.map_err(|e| e.to_string()),
        };
        let status = match stop {
            Ok(status) => Ok(status),
            Err(reason) => {
                let killed = child.start_kill();
                let settled = tokio::time::timeout(Duration::from_millis(TIMEOUT_MS), child.wait()).await;
                if killed.is_err() || !matches!(settled, Ok(Ok(_))) {
                    Err(format!("{reason}; Obsidian process settlement unknown"))
                } else {
                    Err(reason)
                }
            }
        };
        // Both pipe owners settle even on kill/timeout. No detached read or
        // unjoined process is substituted by a successful command receipt.
        let (mut stdout_result, mut stderr_result) = (None, None);
        let deadline = tokio::time::sleep(Duration::from_secs(2));
        tokio::pin!(deadline);
        while stdout_result.is_none() || stderr_result.is_none() {
            tokio::select! {
                result = &mut out, if stdout_result.is_none() => stdout_result = Some(result),
                result = &mut err, if stderr_result.is_none() => stderr_result = Some(result),
                _ = &mut deadline => {
                    if stdout_result.is_none() { out.abort(); let _ = out.await; }
                    if stderr_result.is_none() { err.abort(); let _ = err.await; }
                    return Err(error(format!("{}; Obsidian output settlement unknown", status.err().unwrap_or_else(|| "Obsidian process exited".into()))));
                },
            }
        }
        let stdout = stdout_result.expect("settled stdout owner").map_err(error)?.map_err(error)?;
        let stderr = stderr_result.expect("settled stderr owner").map_err(error)?.map_err(error)?;
        let status = status.map_err(error)?;
        let stdout = String::from_utf8_lossy(&stdout).into_owned();
        let stderr = String::from_utf8_lossy(&stderr).into_owned();
        let reported = if stderr.trim().starts_with("Error:") {
            Some(stderr.trim())
        } else if stdout.trim().starts_with("Error:") {
            Some(stdout.trim())
        } else {
            None
        };
        if !status.success() || reported.is_some() {
            let detail = reported
                .or_else(|| (!stderr.trim().is_empty()).then(|| stderr.trim()))
                .or_else(|| (!stdout.trim().is_empty()).then(|| stdout.trim()))
                .map(str::to_owned)
                .unwrap_or_else(|| format!("obsidian exited with code {}", status.code().unwrap_or(-1)));
            return Err(error(format!("vault://{label} failed: {detail}")));
        }
        Ok(stdout)
    }
    fn cached_root(&self, parsed: &VaultUrl) -> Option<PathBuf> {
        let cache = lock(&self.process.cache);
        if parsed.active { cache.active.clone() } else { cache.directory.as_ref()?.get(&parsed.name).cloned() }
    }
    async fn directory(&self, cancel: CancellationToken) -> Result<BTreeMap<String, PathBuf>, ToolError> {
        if let Some(directory) = lock(&self.process.cache).directory.clone() {
            return Ok(directory);
        }
        let _discovery = self.process.discovery.lock().await;
        if let Some(directory) = lock(&self.process.cache).directory.clone() {
            return Ok(directory);
        }
        let source = self.spawn(&["vaults".into(), "verbose".into()], "vaults", cancel).await?;
        let mut directory = BTreeMap::new();
        let mut order = Vec::new();
        for line in source.lines() {
            if let Some((name, path)) =
                line.trim_end().split_once('\t').filter(|(name, path)| !name.is_empty() && !path.trim().is_empty())
            {
                if !directory.contains_key(name) {
                    order.push(name.to_owned());
                }
                directory.insert(name.into(), std::path::absolute(path.trim()).map_err(error)?);
            }
        }
        {
            let mut cache = lock(&self.process.cache);
            cache.directory = Some(directory.clone());
            cache.directory_order = order;
        }
        Ok(directory)
    }
    async fn root(&self, parsed: &VaultUrl, cancel: CancellationToken) -> Result<PathBuf, ToolError> {
        if let Some(root) = self.cached_root(parsed) {
            return Ok(root);
        }
        if parsed.active {
            let _discovery = self.process.discovery.lock().await;
            if let Some(root) = self.cached_root(parsed) {
                return Ok(root);
            }
            let source = self.spawn(&["vault".into(), "info".into(), "path".into()], "vault info path", cancel).await?;
            let path = source
                .lines()
                .find_map(|line| {
                    let (name, path) = line.trim().split_once('\t').or_else(|| line.trim().split_once(':'))?;
                    name.eq_ignore_ascii_case("path").then(|| path.trim().to_owned())
                })
                .unwrap_or_else(|| if source.trim().contains('\n') { String::new() } else { source.trim().into() });
            if path.is_empty() {
                return Err(error("vault:// active vault path was empty"));
            }
            let root = std::path::absolute(path).map_err(error)?;
            lock(&self.process.cache).active = Some(root.clone());
            Ok(root)
        } else {
            let directory = self.directory(cancel).await?;
            directory.get(&parsed.name).cloned().ok_or_else(|| {
                let mut names = directory.keys().cloned().collect::<Vec<_>>();
                names.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
                error(format!(
                    "Unknown Obsidian vault: {}\nAvailable: {}",
                    parsed.name,
                    if names.is_empty() { "none".into() } else { names.join(", ") }
                ))
            })
        }
    }
    async fn fs_target(&self, parsed: &VaultUrl, cancel: CancellationToken) -> Result<(PathBuf, PathBuf), ToolError> {
        let root = tokio::fs::canonicalize(self.root(parsed, cancel).await?).await.map_err(error)?;
        let target = root.join(&parsed.relative);
        within(&target, &root)?;
        Ok((root, target))
    }
    /// Native EDIT/AST path conversion cannot discover a vault through the CLI.
    pub fn resolve_cached_path(&self, url: &str) -> Result<PathBuf, ToolError> {
        self.check_enabled()?;
        let parsed = parse(url)?;
        if !matches!(parsed.kind, Kind::File | Kind::Directory) {
            return Err(error("vault:// path resolution only supports plain filesystem paths"));
        }
        let root =
            std::fs::canonicalize(self.cached_root(&parsed).ok_or_else(|| error(MISSING_ROOT))?).map_err(error)?;
        let target = root.join(&parsed.relative);
        within(&target, &root)?;
        match std::fs::canonicalize(&target) {
            Ok(real) => within(&real, &root)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                within(&existing_ancestor(target.parent().unwrap_or(&root), &root)?, &root)?
            }
            Err(e) => return Err(error(e)),
        }
        Ok(target)
    }
    async fn plain_file(&self, parsed: &VaultUrl, cancel: CancellationToken) -> Result<(PathBuf, bool), ToolError> {
        let (root, target) = self.fs_target(parsed, cancel).await?;
        match tokio::fs::canonicalize(target.parent().unwrap_or(&root)).await {
            Ok(parent) => within(&parent, &root)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(error(e)),
        }
        let real = match tokio::fs::canonicalize(&target).await {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(error(format!("Vault file not found: {}", parsed.url)));
            }
            result => result.map_err(error)?,
        };
        within(&real, &root)?;
        let metadata = tokio::fs::metadata(&real).await.map_err(error)?;
        if !metadata.is_dir() && !metadata.is_file() {
            return Err(error(format!("vault:// URL must resolve to a file or directory: {}", parsed.url)));
        }
        Ok((real, metadata.is_dir()))
    }
    async fn list_dir(&self, parsed: &VaultUrl, cancel: CancellationToken) -> Result<UriResource, ToolError> {
        let (root, target) = self.fs_target(parsed, cancel).await?;
        let target = tokio::fs::canonicalize(target).await.map_err(error)?;
        within(&target, &root)?;
        if !tokio::fs::metadata(&target).await.map_err(error)?.is_dir() {
            return Err(error(format!("vault:// URL must resolve to a directory: {}", parsed.url)));
        }
        let mut directory = tokio::fs::read_dir(target).await.map_err(error)?;
        let mut entries = Vec::new();
        while let Some(entry) = directory.next_entry().await.map_err(error)? {
            entries.push((
                entry.file_name().to_string_lossy().into_owned(),
                entry.file_type().await.map_err(error)?.is_dir(),
            ));
        }
        let collation = crate::catalog_discovery::CatalogCollation::for_host().map_err(error)?;
        entries.sort_by(|a, b| collation.compare(&a.0.as_str().into(), &b.0.as_str().into()));
        let lines = entries
            .iter()
            .map(|(name, dir)| {
                let relative =
                    if parsed.relative.is_empty() { name.clone() } else { format!("{}/{name}", parsed.relative) };
                format!("- [{name}{}]({})", if *dir { "/" } else { "" }, parsed.link(&relative, *dir))
            })
            .collect::<Vec<_>>();
        let listing = if lines.is_empty() { "(empty)".into() } else { lines.join("\n") };
        let title = if parsed.relative.is_empty() { "/".into() } else { format!("/{}/", parsed.relative) };
        Ok(resource(
            format!(
                "# Vault {}{title}\n\n{} entr{}:\n\n{listing}\n",
                parsed.name,
                entries.len(),
                if entries.len() == 1 { "y" } else { "ies" }
            ),
            "text/markdown",
            true,
        ))
    }
}
fn resource(content: String, kind: &str, immutable: bool) -> UriResource {
    UriResource { content, content_type: kind.into(), notes: Vec::new(), immutable }
}
fn within(target: &Path, root: &Path) -> Result<(), ToolError> {
    if target == root || target.starts_with(root) { Ok(()) } else { Err(error("vault:// URL escapes vault root")) }
}
fn existing_ancestor(target: &Path, root: &Path) -> Result<PathBuf, ToolError> {
    let mut current = target.to_path_buf();
    loop {
        within(&current, root)?;
        match std::fs::canonicalize(&current) {
            Ok(real) => return Ok(real),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if !current.pop() {
                    return Err(error(e));
                }
            }
            Err(e) => return Err(error(e)),
        }
    }
}
fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|s| s.to_str()).unwrap_or_default().to_ascii_lowercase().as_str() {
        "md" | "markdown" | "mdown" | "mkd" | "mdx" | "mdc" => "text/markdown",
        "json" => "application/json",
        _ => "text/plain",
    }
}
fn find_binary() -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let candidates = if cfg!(windows) {
        vec!["obsidian.exe", "obsidian.com", "obsidian.cmd", "obsidian.bat", "obsidian"]
    } else {
        vec!["obsidian"]
    };
    for directory in std::env::split_paths(&path) {
        for name in &candidates {
            let file = directory.join(name);
            if file.is_file() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt as _;
                    if std::fs::metadata(&file).ok()?.permissions().mode() & 0o111 == 0 {
                        continue;
                    }
                }
                return std::path::absolute(file).ok();
            }
        }
    }
    if cfg!(target_os = "macos") && Path::new(DARWIN_BINARY).is_file() { Some(DARWIN_BINARY.into()) } else { None }
}
#[async_trait]
impl ContentUriPort for HostVault {
    fn edit_specs(&self)->Vec<ara_tools::ContentUriEditSpec> {
        vec![ara_tools::ContentUriEditSpec {scheme:"vault".into(),single_slash_alias:false,line_selectors:true,port_authority:false,immutable:false,policy:ara_tools::ContentUriEditPolicy::File {plan_writable:false}}]
    }
    async fn resolve_edit_file(&self,url:&str,cancel:CancellationToken)->Result<Option<UriFileResource>,ToolError> {
        self.check_enabled()?;cancelled(&cancel)?;
        let parsed=parse(url)?;
        if parsed.kind!=Kind::File {return Ok(None);}
        // Unlike read, locate(create:true) may return a missing file. Discovery
        // is Host-owned; the returned target must stay in the physical root.
        let (root,target)=self.fs_target(&parsed,cancel.clone()).await?;
        ara_tools::edit_urls::validate_containment(&target,&root)?;
        cancelled(&cancel)?;
        Ok(Some(UriFileResource {path:target,content_type:"text/plain".into(),max_inline_bytes:None,immutable:false}))
    }
    fn route(&self, url: &str) -> ContentUriRoute {
        if ara_tools::internal_urls::hierarchical_scheme(url).is_some_and(|scheme| scheme.eq_ignore_ascii_case("vault"))
        {
            ContentUriRoute::Registered { writable: true }
        } else {
            ContentUriRoute::Unregistered
        }
    }
    async fn read_file(&self, url: &str, cancel: CancellationToken) -> Result<Option<UriFileResource>, ToolError> {
        self.check_enabled()?;
        cancelled(&cancel)?;
        let parsed = parse(url)?;
        if parsed.kind != Kind::File {
            return Ok(None);
        }
        let (path, directory) = self.plain_file(&parsed, cancel).await?;
        Ok((!directory).then(|| UriFileResource {
            content_type: content_type(&path).into(),
            path,
            max_inline_bytes: None,
            immutable: false,
        }))
    }
    async fn search_path_only(&self, url: &str, cancel: CancellationToken) -> Result<Option<PathBuf>, ToolError> {
        self.check_enabled()?;
        cancelled(&cancel)?;
        let parsed = parse(url)?;
        if !matches!(parsed.kind, Kind::File | Kind::Directory) {
            return Ok(None);
        }
        let target = self.resolve_cached_path(url)?;
        let root =
            std::fs::canonicalize(self.cached_root(&parsed).ok_or_else(|| error(MISSING_ROOT))?).map_err(error)?;
        let real = tokio::fs::canonicalize(target).await.map_err(error)?;
        within(&real, &root)?;
        let metadata = tokio::fs::metadata(&real).await.map_err(error)?;
        if !metadata.is_dir() && !metadata.is_file() {
            return Err(error(format!("vault:// URL must resolve to a file or directory: {}", parsed.url)));
        }
        Ok(Some(real))
    }
    async fn read(&self, url: &str, cancel: CancellationToken) -> Result<UriResource, ToolError> {
        self.check_enabled()?;
        cancelled(&cancel)?;
        let parsed = parse(url)?;
        match parsed.kind {
            Kind::List => {
                let directory = self.directory(cancel).await?;
                let mut names = directory.keys().collect::<Vec<_>>();
                let collation = crate::catalog_discovery::CatalogCollation::for_host().map_err(error)?;
                names.sort_by(|a, b| collation.compare(&a.as_str().into(), &b.as_str().into()));
                let lines = names
                    .into_iter()
                    .map(|name| format!("- [{name}](vault://{}/)", encode_host(name)))
                    .collect::<Vec<_>>();
                Ok(resource(
                    format!(
                        "# Obsidian Vaults\n\n{} vault{} available:\n\n{}\n",
                        directory.len(),
                        if directory.len() == 1 { "" } else { "s" },
                        if lines.is_empty() { "(none)".into() } else { lines.join("\n") }
                    ),
                    "text/markdown",
                    true,
                ))
            }
            Kind::Info => {
                let root = self.root(&parsed, cancel.clone()).await?;
                let mut info = lock(&self.process.cache).info.get(&parsed.name).cloned();
                if info.is_none() {
                    let _discovery = self.process.discovery.lock().await;
                    info = lock(&self.process.cache).info.get(&parsed.name).cloned();
                    if info.is_none() {
                        let mut args = parsed.vault_args();
                        args.extend(["vault".into(), "info".into()]);
                        let result = self.spawn(&args, "vault info", cancel).await?;
                        info = Some(result.trim().to_owned());
                        lock(&self.process.cache).info.insert(parsed.name.clone(), result.trim().into());
                    }
                }
                let (files, folders) = tokio::task::spawn_blocking({
                    let root = root.clone();
                    move || count_entries(&root)
                })
                .await
                .map_err(error)??;
                let content = serde_json::to_string_pretty(&json!({"name":parsed.name,"rootPath":root,"files":files,"folders":folders,"info":info.unwrap_or_default()})).map_err(error)? + "\n";
                Ok(resource(content, "application/json", true))
            }
            Kind::Directory => self.list_dir(&parsed, cancel).await,
            Kind::File => {
                let (path, directory) = self.plain_file(&parsed, cancel.clone()).await?;
                if directory {
                    return self.list_dir(&parsed, cancel).await;
                }
                let bytes = tokio::fs::read(&path).await.map_err(error)?;
                Ok(resource(String::from_utf8_lossy(&bytes).into_owned(), content_type(&path), false))
            }
            Kind::FileOp | Kind::VaultOp => {
                let (args, content_type) = invocation(&parsed)?;
                let content = self.spawn(&args, parsed.op.as_deref().unwrap_or_default(), cancel).await?;
                Ok(resource(content, content_type, true))
            }
        }
    }
    async fn write(&self, url: &str, content: &str, cancel: CancellationToken) -> Result<(), ToolError> {
        self.check_enabled()?;
        cancelled(&cancel)?;
        let parsed = parse(url)?;
        if parsed.kind != Kind::File {
            return Err(error("vault:// write only supports plain file paths"));
        }
        let (root, target) = self.fs_target(&parsed, cancel.clone()).await?;
        match tokio::fs::canonicalize(&target).await {
            Ok(real) => {
                within(&real, &root)?;
                if tokio::fs::metadata(real).await.map_err(error)?.is_dir() {
                    return Err(error(format!("vault:// URL must resolve to a file: {}", parsed.url)));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let parent = target.parent().unwrap_or(&root).to_path_buf();
                let ancestor = tokio::task::spawn_blocking({
                    let parent = parent.clone();
                    let root = root.clone();
                    move || existing_ancestor(&parent, &root)
                })
                .await
                .map_err(error)??;
                within(&ancestor, &root)?;
                cancelled(&cancel)?;
                tokio::fs::create_dir_all(&parent).await.map_err(error)?;
                within(&tokio::fs::canonicalize(parent).await.map_err(error)?, &root)?;
            }
            Err(e) => return Err(error(e)),
        }
        cancelled(&cancel)?;
        // Once the write is dispatched its original owner settles; cancelling
        // a model cannot replace it with a fresh write or an invented no-effect.
        tokio::fs::write(target, content.as_bytes()).await.map_err(error)
    }
}
fn count_entries(root: &Path) -> Result<(usize, usize), ToolError> {
    let mut pending = vec![root.to_path_buf()];
    let (mut files, mut folders) = (0, 0);
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory).map_err(error)? {
            let entry = entry.map_err(error)?;
            let kind = entry.file_type().map_err(error)?;
            if kind.is_dir() {
                folders += 1;
                pending.push(entry.path());
            } else if kind.is_file() {
                files += 1;
            }
        }
    }
    Ok((files, folders))
}

/// Preserve registered/removed precedence through ACP, native vault, artifacts
/// and the existing Host URI bridge. A missing handler remains unregistered.
pub fn compose_ports(
    first: Arc<dyn ContentUriPort>,
    second: Option<Arc<dyn ContentUriPort>>,
) -> Arc<dyn ContentUriPort> {
    match second {
        Some(second) => Arc::new(UriChain { first, second }),
        None => first,
    }
}
struct UriChain {
    first: Arc<dyn ContentUriPort>,
    second: Arc<dyn ContentUriPort>,
}
impl UriChain {
    fn selected(&self, url: &str) -> &Arc<dyn ContentUriPort> {
        if self.first.route(url) == ContentUriRoute::Unregistered { &self.second } else { &self.first }
    }
}
#[async_trait]
impl ContentUriPort for UriChain {
    fn edit_specs(&self)->Vec<ara_tools::ContentUriEditSpec> {
        let mut specs=self.first.edit_specs();
        for spec in self.second.edit_specs() {
            if !specs.iter().any(|first|first.scheme.eq_ignore_ascii_case(&spec.scheme)) {specs.push(spec);}
        }
        specs
    }
    fn edit_sandbox_roots(&self)->Vec<(String,PathBuf)> {
        let specs=self.first.edit_specs();
        let mut roots=self.first.edit_sandbox_roots();
        roots.extend(self.second.edit_sandbox_roots().into_iter().filter(|(scheme,_)|!specs.iter().any(|spec|spec.scheme.eq_ignore_ascii_case(scheme))));
        roots
    }
    async fn resolve_edit_file(&self,url:&str,cancel:CancellationToken)->Result<Option<UriFileResource>,ToolError> {
        self.selected(url).resolve_edit_file(url,cancel).await
    }
    fn normalize_url(&self, input: &str) -> String {
        ara_tools::internal_urls::normalize_port_fallback(self.first.as_ref(), self.second.as_ref(), input)
    }
    fn locate_sync(&self, url: &str) -> Result<Option<PathBuf>, ToolError> {
        self.selected(url).locate_sync(url)
    }
    fn route(&self, url: &str) -> ContentUriRoute { self.selected(url).route(url) }
    fn write_policy(&self, url: &str) -> ara_tools::ContentUriWritePolicy { self.selected(url).write_policy(url) }
    fn write_selection(&self,url:&str)->ara_tools::ContentUriWriteSelection {self.selected(url).write_selection(url)}
    async fn write_result_selected(&self,url:&str,content:&str,selection:&ara_tools::ContentUriWriteSelection,cancel:CancellationToken)->Result<Option<ToolOutput>,ara_tools::ContentUriWriteDispatchError> {
        let selected=self.selected(url).clone();
        if &selected.write_selection(url)!=selection{return Err(ara_tools::ContentUriWriteDispatchError::NotDispatched(ToolError("URI chain selected leaf changed; not dispatched".into())));}
        selected.write_result_selected(url,content,selection,cancel).await
    }
    async fn read_file(&self, url: &str, cancel: CancellationToken) -> Result<Option<UriFileResource>, ToolError> {
        self.selected(url).read_file(url, cancel).await
    }
    async fn search_path_only(&self, url: &str, cancel: CancellationToken) -> Result<Option<PathBuf>, ToolError> {
        self.selected(url).search_path_only(url, cancel).await
    }
    async fn read(&self, url: &str, cancel: CancellationToken) -> Result<UriResource, ToolError> {
        self.selected(url).read(url, cancel).await
    }
    async fn write(&self, url: &str, content: &str, cancel: CancellationToken) -> Result<(), ToolError> {
        self.selected(url).write(url, content, cancel).await
    }
    async fn write_result(
        &self,
        url: &str,
        content: &str,
        cancel: CancellationToken,
    ) -> Result<Option<ToolOutput>, ToolError> {
        self.selected(url).write_result(url, content, cancel).await
    }
}
