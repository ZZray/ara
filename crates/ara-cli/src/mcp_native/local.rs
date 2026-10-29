//! Native OS body of fixed internal-urls/local-protocol.ts resolveLocalTarget.
//! The graph driver resolves live option callbacks before building this plan.
//! No Session/graph handle/runtime is captured by the filesystem worker.

use ara_mcp::native_graph::{get, method};
use ara_rpc::value_heap::{ValueError, ValueHandle, ValueHeap, ValueRuntime, ValueSnapshot};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Default, Clone)]
pub struct LocalRoutes {
    /// Host-selected original global override and first main SessionManager.
    /// These original values are graph-driver imports, not worker state.
    pub override_options: Option<ValueHandle>,
    pub main_session_manager: Option<ValueHandle>,
}
pub struct LocalPlan {
    root: PathBuf,
    input: String,
}
impl LocalPlan {
    /// The Host already bound this root from the actual SessionArtifacts/ID.
    /// Builtin READ reuses the same resolver without a new ambient root.
    pub(crate) fn for_session_root(root: PathBuf, input: String) -> Self { Self { root, input } }
}

fn optional_call(
    heap: &Arc<ValueHeap>,
    object: &ValueHandle,
    name: &str,
    rt: &dyn ValueRuntime,
) -> Result<Option<ValueHandle>, ValueError> {
    let function = get(heap, object, name, rt)?;
    if matches!(heap.snapshot(&function)?, ValueSnapshot::Undefined | ValueSnapshot::Null) {
        return Ok(None);
    }
    heap.call(&function, object, &[], rt).map(Some)
}
fn primitive_path(heap: &Arc<ValueHeap>, value: &ValueHandle) -> Result<String, ValueError> {
    let ValueSnapshot::String(value) = heap.snapshot(value)? else {
        return Err(ValueError::TypeError("The path argument must be of type string".into()));
    };
    value.to_utf8().map_err(|_| ValueError::TypeError("Native local path has unpaired UTF16 code units".into()))
}
pub fn prepare(
    heap: &Arc<ValueHeap>,
    context: &ValueHandle,
    routes: &LocalRoutes,
    input: String,
    rt: &dyn ValueRuntime,
) -> Result<Option<LocalPlan>, ValueError> {
    // Original call-site argument evaluation must remain observable even though
    // cwd/settings are not consumed by the current LocalProtocolHandler.
    let manager = get(heap, context, "sessionManager", rt)?;
    if !matches!(heap.snapshot(&manager)?, ValueSnapshot::Undefined | ValueSnapshot::Null) {
        optional_call(heap, &manager, "getCwd", rt)?;
    }
    get(heap, context, "settings", rt)?;
    let supplied = get(heap, context, "localProtocolOptions", rt)?;
    for value in [&routes.override_options, &routes.main_session_manager].into_iter().flatten() {
        heap.adopt(value)?;
    }
    let explicit = if heap.truthy(&supplied)? {
        Some(supplied)
    } else if let Some(value) = &routes.override_options {
        if heap.truthy(value)? { Some(value.clone()) } else { None }
    } else {
        None
    };
    let options = if let Some(options) = explicit { Some(options) } else { None };
    let artifacts = if let Some(options) = &options {
        optional_call(heap, options, "getArtifactsDir", rt)?.unwrap_or(heap.undefined()?)
    } else if let Some(manager) = &routes.main_session_manager {
        method(heap, manager, "getArtifactsDir", &[], rt)?
    } else {
        return Ok(None);
    };
    // Compute the actual resolved candidate before deciding whether getSessionId
    // is needed. Relative paths and callback changes to cwd stay in source order.
    let candidate = if heap.truthy(&artifacts)? {
        Some(
            std::path::absolute(PathBuf::from(primitive_path(heap, &artifacts)?).join("local"))
                .map_err(|error| ValueError::TypeError(error.to_string()))?,
        )
    } else {
        None
    };
    let needs_short = candidate
        .as_ref()
        .is_none_or(|root| cfg!(windows) && root.as_os_str().to_string_lossy().encode_utf16().count() >= 180);
    let root = if needs_short {
        let id = if let Some(options) = &options {
            optional_call(heap, options, "getSessionId", rt)?.unwrap_or(heap.undefined()?)
        } else if let Some(manager) = &routes.main_session_manager {
            method(heap, manager, "getSessionId", &[], rt)?
        } else {
            heap.undefined()?
        };
        short_root(heap, &id)?
    } else {
        candidate.ok_or(ValueError::InvalidNode)?
    };
    Ok(Some(LocalPlan { root, input }))
}
fn short_root(heap: &Arc<ValueHeap>, id: &ValueHandle) -> Result<PathBuf, ValueError> {
    let raw = if matches!(heap.snapshot(id)?, ValueSnapshot::Undefined | ValueSnapshot::Null) {
        "session".to_owned()
    } else {
        primitive_path(heap, id)?
    };
    let safe: String = raw
        .encode_utf16()
        .map(|unit| {
            char::from_u32(u32::from(unit))
                .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
                .unwrap_or('_')
        })
        .collect();
    Ok(std::env::temp_dir().join("omp-local").join(if safe.is_empty() { "session" } else { &safe }))
}
/// Host counterpart of the same local-protocol root rule, using the actual
/// SessionArtifacts directory and Session journal ID. This does not create it.
pub fn session_root(artifacts: Option<&Path>, id: &str) -> std::io::Result<PathBuf> {
    let candidate = artifacts.map(|path| std::path::absolute(path.join("local"))).transpose()?;
    if let Some(root) = candidate
        && !(cfg!(windows) && root.as_os_str().to_string_lossy().encode_utf16().count() >= 180)
    {
        return Ok(root);
    }
    let safe: String = id
        .encode_utf16()
        .map(|unit| {
            char::from_u32(u32::from(unit))
                .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
                .unwrap_or('_')
        })
        .collect();
    Ok(std::env::temp_dir().join("omp-local").join(if safe.is_empty() { "session" } else { &safe }))
}
fn decode(input: &str) -> Result<String, ()> {
    let mut output = Vec::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut offset = 0;
    while offset < bytes.len() {
        if bytes[offset] == b'%' {
            let hex = |b: Option<&u8>| b.and_then(|b| (*b as char).to_digit(16));
            let (Some(a), Some(b)) = (hex(bytes.get(offset + 1)), hex(bytes.get(offset + 2))) else {
                return Err(());
            };
            output.push((a * 16 + b) as u8);
            offset += 3;
        } else {
            output.push(bytes[offset]);
            offset += 1;
        }
    }
    String::from_utf8(output).map_err(|_| ())
}
fn relative(input: &str) -> Result<String, String> {
    let tail = input.strip_prefix("local://").ok_or_else(|| format!("Invalid URL: {input}"))?;
    let tail = &tail[..tail.find(['?', '#']).unwrap_or(tail.len())];
    let (host, pathname) = tail.split_once('/').map_or((tail, ""), |(host, path)| (host, path));
    // parseInternalUrl decodes rawHost once; extractRelativePath decodes the
    // combined rawHost/rawPathname a second time, retaining that source order.
    let host = decode(host).unwrap_or_else(|_| host.to_owned());
    let combined = if host.is_empty() {
        pathname.to_owned()
    } else if pathname.is_empty() {
        host
    } else {
        format!("{host}/{pathname}")
    };
    if combined.is_empty() {
        return Ok(String::new());
    }
    let decoded =
        decode(&combined.replace('\\', "/")).map_err(|_| format!("Invalid URL encoding in local:// path: {input}"))?;
    if Path::new(&decoded).is_absolute() {
        return Err("Absolute paths are not allowed in local:// URLs".into());
    }
    if decoded.split(['/', '\\']).any(|part| part == "..") {
        return Err("Path traversal (..) is not allowed in local:// URLs".into());
    }
    let separator = if cfg!(windows) { "\\" } else { "/" };
    let normalized = decoded
        .split(if cfg!(windows) { &['/', '\\'][..] } else { &['/'][..] })
        .filter(|part| !part.is_empty() && *part != ".")
        .collect::<Vec<_>>()
        .join(separator);
    if normalized.starts_with("..") || normalized.contains("/..") {
        return Err("Path traversal (..) is not allowed in local:// URLs".into());
    }
    Ok(decoded)
}
fn within(target: &Path, root: &Path) -> Result<(), String> {
    if target != root && !target.starts_with(root) {
        return Err("local:// URL escapes local root".into());
    }
    Ok(())
}
fn normalized_canonical(path: PathBuf) -> PathBuf {
    // Node's fs.realpath and path.resolve operate in the same Windows namespace.
    PathBuf::from(ara_tools::internal_urls::search_path_string(&path))
}
pub async fn resolve(plan: LocalPlan) -> Result<Option<(String, u64)>, String> {
    tokio::fs::create_dir_all(&plan.root).await.map_err(|error| error.to_string())?;
    let root = normalized_canonical(tokio::fs::canonicalize(&plan.root).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            "Unable to initialize local:// root".into()
        } else {
            error.to_string()
        }
    })?);
    let relative = relative(&plan.input)?;
    let target = if relative.is_empty() {
        root.clone()
    } else {
        let separator = if cfg!(windows) { &['/', '\\'][..] } else { &['/'][..] };
        let mut target = root.clone();
        for part in relative.split(separator).filter(|part| !part.is_empty() && *part != ".") {
            target.push(part);
        }
        target
    };
    within(&target, &root)?;
    if target == root {
        return Ok(None);
    }
    if let Some(parent) = target.parent() {
        match tokio::fs::canonicalize(parent).await {
            Ok(parent) => within(&normalized_canonical(parent), &root)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    let target = normalized_canonical(tokio::fs::canonicalize(&target).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            format!("Local file not found: {}", plan.input)
        } else {
            error.to_string()
        }
    })?);
    within(&target, &root)?;
    let stat = tokio::fs::metadata(&target).await.map_err(|error| error.to_string())?;
    if stat.is_dir() {
        return Ok(None);
    }
    if !stat.is_file() {
        return Err(format!("local:// URL must resolve to a file or directory: {}", plan.input));
    }
    Ok(Some((ara_tools::internal_urls::search_path_string(&target), stat.len())))
}


// Fixed OMP local-protocol.ts, MIT; see THIRD_PARTY_NOTICES.md.
// Copyright (c) 2025-2026 Can Bölük; (c) 2026 Stencil Labs, Inc.
/// Direct fixed LocalProtocol file resource. Root listings/directories are
/// deliberately not represented by this bounded API. The caller supplies the
/// parsed InternalUrl href; URI registry precedence remains in Native READ.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalFileResource {
    pub url: String,
    pub content: String,
    pub content_type: String,
    pub size: usize,
    pub source_path: String,
    pub notes: Vec<String>,
}

const LOCAL_WRITE_NOTE: &str = "Use write path local://<file> to persist large intermediate artifacts across turns.";
const LOCAL_TEXT_SNIFF_BYTES: u64 = 8192;
const LOCAL_TEXT_RESOURCE_MAX_BYTES: u64 = 1048576;

// Exact binary64 toFixed(1), including JS ties-up and the retained .0.
// Byte-size quotients supplied here are positive and at most 2^34.
fn local_fixed_one(value: f64) -> String {
    let bits = value.to_bits();
    let significand = u128::from((bits & ((1u64 << 52) - 1)) | (1u64 << 52));
    let shift = (1075 - ((bits >> 52) & 0x7ff)) as u32;
    let numerator = significand * 10;
    let integral = (numerator >> shift)
        + u128::from((numerator & ((1u128 << shift) - 1)) >= (1u128 << (shift - 1)));
    format!("{}.{}", integral / 10, integral % 10)
}
fn local_byte_size(bytes: u64) -> String {
    if bytes < 1024 { return format!("{bytes} B"); }
    let kib = bytes as f64 / 1024.0;
    if kib < 1024.0 { return format!("{} KiB", local_fixed_one(kib)); }
    let mib = kib / 1024.0;
    if mib < 1024.0 { return format!("{} MiB", local_fixed_one(mib)); }
    format!("{} GiB", local_fixed_one(mib / 1024.0))
}
fn local_content_type(path: &str) -> &'static str {
    let base = Path::new(path).file_name().unwrap_or_default().to_string_lossy().to_lowercase();
    // getLanguageFromPath special basenames precede the theme extension key.
    let special = base.starts_with(".env.") || base == "dockerfile" || base.starts_with("dockerfile.")
        || matches!(base.as_str(), "containerfile" | ".emacs" | "justfile" | "cmakelists.txt");
    let key = path.rsplit_once('.').map_or(path, |(_, key)| key).to_lowercase();
    if !special && matches!(key.as_str(), "md" | "markdown" | "mdx" | "mdc" | "mkd" | "mdown") {
        "text/markdown"
    } else if Path::new(path).extension().is_some_and(|ext| ext.to_string_lossy().eq_ignore_ascii_case("json")) {
        "application/json"
    } else {
        "text/plain"
    }
}
fn local_resource(href: String, path: String, content: String, content_type: &str) -> LocalFileResource {
    LocalFileResource { url: href, size: content.len(), content, content_type: content_type.into(),
        source_path: path, notes: vec![LOCAL_WRITE_NOTE.into()] }
}
fn non_text_resource(href: String, path: String, size: u64, reason: &str) -> LocalFileResource {
    let content = format!("[Cannot read binary local:// file '{href}' ({}): {reason}. This resource is not text. Use a metadata/key-frame/video-specific workflow instead.]", local_byte_size(size));
    local_resource(href, path, content, "text/plain")
}

/// Reuses the unchanged checked Native path resolver; it never promotes the
/// notice to a tool result or installs a competing content URI registration.
/// Source: fixed local-protocol.ts buildFileResource and its direct dependencies.
pub async fn resolve_file_resource(plan: LocalPlan) -> Result<Option<LocalFileResource>, String> {
    use tokio::io::AsyncReadExt;
    let href = plan.input.clone();
    let Some((path, size)) = resolve(plan).await? else { return Ok(None); };
    let ext = Path::new(&path).extension().unwrap_or_default().to_string_lossy().to_lowercase();
    if matches!(ext.as_str(), "7z" | "avi" | "bmp" | "bz2" | "db" | "doc" | "docx" | "gif"
        | "gz" | "ico" | "jpeg" | "jpg" | "m4v" | "mkv" | "mov" | "mp4" | "pdf" | "png"
        | "ppt" | "pptx" | "rar" | "sqlite" | "tgz" | "webm" | "webp" | "wmv" | "xls"
        | "xlsx" | "xz" | "zip") {
        return Ok(Some(non_text_resource(href, path, size, "extension is a known binary/container type")));
    }
    let mut file = tokio::fs::File::open(&path).await.map_err(|error| error.to_string())?;
    let mut prefix = vec![0; size.min(LOCAL_TEXT_SNIFF_BYTES) as usize];
    let length = file.read(&mut prefix).await.map_err(|error| error.to_string())?;
    prefix.truncate(length);
    drop(file);
    if prefix.contains(&0) || std::str::from_utf8(&prefix).is_err() {
        return Ok(Some(non_text_resource(href, path, size, "content is not valid UTF-8 text")));
    }
    if size > LOCAL_TEXT_RESOURCE_MAX_BYTES {
        let content = format!("[Cannot materialize local:// file '{href}' as an internal text resource ({} exceeds {}). Use the read tool's filesystem path handling or a line selector so content is streamed with file-size safeguards.]",
            local_byte_size(size), local_byte_size(LOCAL_TEXT_RESOURCE_MAX_BYTES));
        return Ok(Some(local_resource(href, path, content, "text/plain")));
    }
    let bytes = tokio::fs::read(&path).await.map_err(|error| error.to_string())?;
    // Bun.file.text uses replacement decoding, unlike the fatal prefix sniff.
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
    let content = String::from_utf8_lossy(bytes).into_owned();
    let content_type = local_content_type(&path);
    Ok(Some(local_resource(href, path, content, content_type)))
}
