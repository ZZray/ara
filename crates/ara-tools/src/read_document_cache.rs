//! Fixed OMP596f2da markit conversion cache; Host owns its storage root.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.

use ara_rpc::{WireString, WireValue};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub const CACHE_VERSION: u32 = 1;
pub const CONVERTER_VERSION: &str = "18.1.8";
pub const MAX_CACHE_BYTES: u64 = 256 * 1024 * 1024;
const TMP_ORPHAN_MAX_AGE: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone)]
pub struct DocumentCache {
    root: PathBuf,
    converter_version: String,
}
pub type DocumentConversionCache = DocumentCache;

impl DocumentCache {
    pub fn new(root: PathBuf, converter_version: String) -> Self {
        Self { root, converter_version }
    }

    /// The package version invalidates converter output independently of schema.
    pub fn key(&self, extension: &str, source: &[u8]) -> String {
        let normalized = trim_js(extension).to_lowercase();
        let normalized = normalized.trim_start_matches('.');
        let normalized = if normalized.is_empty() { "bin" } else { normalized };
        let extension = safe_component(normalized, false);
        let extension = if extension.is_empty() { "bin" } else { &extension };
        let version = safe_component(&self.converter_version, true);
        let digest = Sha256::digest(source);
        format!("v{CACHE_VERSION}-{version}-{extension}-{digest:x}")
    }

    /// Ordinary Rust markdown boundary. Use get_wire to retain unpaired UTF16.
    pub fn get(&self, extension: &str, source: &[u8]) -> Option<String> {
        self.get_wire(extension, source).map(|content| String::from_utf16_lossy(content.units()))
    }

    pub fn get_wire(&self, extension: &str, source: &[u8]) -> Option<WireString> {
        let target = self.entry_path(extension, source);
        let bytes = fs::read(&target).ok()?;
        let raw = String::from_utf8_lossy(&bytes);
        let entry = WireValue::parse(&raw).ok().and_then(|value| {
            if value.get("version")?.as_number()? != f64::from(CACHE_VERSION) {
                return None;
            }
            let content = value.get("content")?.as_string()?;
            (!content.is_empty()).then(|| content.clone())
        });
        if entry.is_none() {
            let _ = remove_file_force(&target);
        }
        entry
    }

    pub fn put(&self, extension: &str, source: &[u8], content: &str) {
        self.put_wire(extension, source, &WireString::from(content));
    }

    pub fn put_wire(&self, extension: &str, source: &[u8], content: &WireString) {
        let key = self.key(extension, source);
        let target = self.root.join(format!("{key}.json"));
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis();
        let temp = self.root.join(format!("{key}.{}.{now}.{}.tmp", std::process::id(), uuid::Uuid::new_v4()));
        let payload = WireValue::object(vec![
            ("version", WireValue::Number(f64::from(CACHE_VERSION))),
            ("content", WireValue::String(content.clone())),
        ])
        .stringify();
        let write = || -> std::io::Result<()> {
            fs::create_dir_all(&self.root)?;
            fs::write(&temp, payload.as_bytes())?;
            // std::fs::rename replaces atomically on the supported Windows/Unix
            // hosts; never unlink the destination before this operation.
            fs::rename(&temp, &target)
        };
        if write().is_err() {
            let _ = remove_file_force(&temp);
            return;
        }
        // Bun's unawaited async prune becomes an independent synchronous worker.
        // The caller only waits for publish; worker creation and GC are nonfatal.
        let root = self.root.clone();
        let _ = std::thread::Builder::new().name("ara-document-cache-prune".into()).spawn(move || prune_root(&root));
    }

    pub fn prune(&self) {
        prune_root(&self.root);
    }

    fn entry_path(&self, extension: &str, source: &[u8]) -> PathBuf {
        self.root.join(format!("{}.json", self.key(extension, source)))
    }
}

fn trim_js(text: &str) -> &str {
    text.trim_matches(|character: char| {
        matches!(character,
        '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' |
        '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' |
        '\u{205f}' | '\u{3000}' | '\u{feff}')
    })
}

fn safe_component(text: &str, uppercase: bool) -> String {
    let mut result = String::new();
    let mut replaced = false;
    for character in text.chars() {
        if character.is_ascii_lowercase() || character.is_ascii_digit() || (uppercase && character.is_ascii_uppercase())
        {
            result.push(character);
            replaced = false;
        } else if !replaced {
            result.push('_');
            replaced = true;
        }
    }
    result
}

fn remove_file_force(path: &Path) -> std::io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

fn prune_root(root: &Path) {
    prune_root_with_max(root, MAX_CACHE_BYTES);
}

fn mtime_ms(time: SystemTime) -> f64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(age) => age.as_secs_f64() * 1000.0,
        Err(age) => -(age.duration().as_secs_f64() * 1000.0),
    }
}

fn prune_root_with_max(root: &Path, maximum: u64) {
    let Ok(names) = fs::read_dir(root) else { return };
    // Date.now is integer milliseconds; stat.mtimeMs retains its fraction.
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as f64;
    let mut entries = Vec::new();
    let mut total = 0_u64;
    for entry in names.flatten() {
        let path = entry.path();
        // metadata follows links, like the fixed source fs.stat.
        let Ok(stat) = fs::metadata(&path) else { continue };
        if !stat.is_file() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Ok(modified) = stat.modified() else { continue };
        let millis = mtime_ms(modified);
        if name.ends_with(".tmp") {
            if now - millis > TMP_ORPHAN_MAX_AGE.as_millis() as f64 {
                let _ = remove_file_force(&path);
            }
            continue;
        }
        if !name.ends_with(".json") {
            continue;
        }
        total += stat.len();
        entries.push((path, stat.len(), millis));
    }
    if total <= maximum {
        return;
    }
    // Stable mtime ordering is FIFO, not LRU; reading never touches mtime.
    entries.sort_by(|left, right| left.2.total_cmp(&right.2));
    for (path, size, _) in entries {
        if total <= maximum {
            break;
        }
        if remove_file_force(&path).is_ok() {
            total -= size;
        }
    }
}

/// Fixed markit.ts wrapper result. Cache status describes conversion, not IO health.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentCacheStatus {
    Hit,
    Miss,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedDocumentConversion {
    pub content: String,
    pub ok: bool,
    pub error: Option<String>,
    pub cache: DocumentCacheStatus,
}

/// Fixed StreamInfo fields used by the buffer/file wrappers and convertFile.
/// Format selection and image extraction remain the injected converter's job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentStreamInfo {
    pub extension: String,
    pub filename: String,
    pub local_path: Option<PathBuf>,
    pub image_dir: Option<PathBuf>,
}

fn normalized_extension(extension: &str) -> String {
    let lowered = trim_js(extension).to_lowercase();
    if lowered.is_empty() {
        ".bin".into()
    } else if lowered.starts_with('.') {
        lowered
    } else {
        format!(".{lowered}")
    }
}

fn conversion_result(
    result: Result<String, ara_agent::ToolError>,
    status: DocumentCacheStatus,
) -> CachedDocumentConversion {
    match result {
        Ok(content) if !content.is_empty() => {
            CachedDocumentConversion { content, ok: true, error: None, cache: status }
        }
        Ok(_) => CachedDocumentConversion {
            content: String::new(),
            ok: false,
            error: Some("Conversion produced no output".into()),
            cache: status,
        },
        Err(error) => {
            let message = trim_js(&error.0);
            CachedDocumentConversion {
                content: String::new(),
                ok: false,
                error: Some(if message.is_empty() { "Conversion failed".into() } else { message.into() }),
                cache: status,
            }
        }
    }
}

fn check_conversion_cancel(cancel: &tokio_util::sync::CancellationToken) -> Result<(), ara_agent::ToolError> {
    if cancel.is_cancelled() { Err(ara_agent::ToolError("Document conversion was aborted".into())) } else { Ok(()) }
}

fn run_cached_buffer_conversion<F>(
    cache: Option<&DocumentCache>,
    bytes: &[u8],
    stream: &DocumentStreamInfo,
    cancel: &tokio_util::sync::CancellationToken,
    cache_enabled: bool,
    convert: F,
) -> Result<CachedDocumentConversion, ara_agent::ToolError>
where
    F: FnOnce(&[u8], &DocumentStreamInfo) -> Result<String, ara_agent::ToolError>,
{
    let status = if cache_enabled { DocumentCacheStatus::Miss } else { DocumentCacheStatus::Skipped };
    if cache_enabled {
        check_conversion_cancel(cancel)?;
        let cached = cache.and_then(|cache| cache.get(&stream.extension, bytes));
        check_conversion_cancel(cancel)?;
        if let Some(content) = cached {
            return Ok(CachedDocumentConversion { content, ok: true, error: None, cache: DocumentCacheStatus::Hit });
        }
    }
    check_conversion_cancel(cancel)?;
    let result = convert(bytes, stream);
    // The synchronous adapter runs on READ's existing cancellation-aware worker.
    // Check cancellation before finalizing or publishing the callback result.
    check_conversion_cancel(cancel)?;
    let finalized = conversion_result(result, status);
    if finalized.ok
        && cache_enabled
        && let Some(cache) = cache
    {
        cache.put(&stream.extension, bytes, &finalized.content);
    }
    Ok(finalized)
}

/// Fixed convertBufferWithMarkit. Run on the caller's existing blocking worker.
/// An absent Host cache behaves as a miss; use_cache=false reports skipped.
pub fn convert_buffer_with_cache<F>(
    cache: Option<&DocumentCache>,
    bytes: &[u8],
    extension: &str,
    cancel: &tokio_util::sync::CancellationToken,
    use_cache: bool,
    convert: F,
) -> Result<CachedDocumentConversion, ara_agent::ToolError>
where
    F: FnOnce(&[u8], &DocumentStreamInfo) -> Result<String, ara_agent::ToolError>,
{
    let extension = normalized_extension(extension);
    let stream =
        DocumentStreamInfo { filename: format!("input{extension}"), extension, local_path: None, image_dir: None };
    run_cached_buffer_conversion(cache, bytes, &stream, cancel, use_cache, convert)
}

/// Fixed convertFileWithMarkit plus Markit.convertFile's bytes/StreamInfo bridge.
/// imageDir bypasses lookup/publication so every request reaches the converter.
pub fn convert_file_with_cache<F>(
    cache: Option<&DocumentCache>,
    file: &Path,
    cancel: &tokio_util::sync::CancellationToken,
    image_dir: Option<&Path>,
    convert: F,
) -> Result<CachedDocumentConversion, ara_agent::ToolError>
where
    F: FnOnce(&[u8], &DocumentStreamInfo) -> Result<String, ara_agent::ToolError>,
{
    let image_dir = image_dir.filter(|path| !path.as_os_str().is_empty());
    let status = if image_dir.is_some() { DocumentCacheStatus::Skipped } else { DocumentCacheStatus::Miss };
    check_conversion_cancel(cancel)?;
    let bytes = match fs::read(file) {
        Ok(bytes) => bytes,
        Err(error) => {
            check_conversion_cancel(cancel)?;
            return Ok(conversion_result(Err(ara_agent::ToolError(error.to_string())), status));
        }
    };
    check_conversion_cancel(cancel)?;
    let stream = DocumentStreamInfo {
        extension: file
            .extension()
            .map(|extension| format!(".{}", extension.to_string_lossy().to_lowercase()))
            .unwrap_or_default(),
        filename: file.file_name().unwrap_or_default().to_string_lossy().into_owned(),
        local_path: Some(file.into()),
        image_dir: image_dir.map(Path::to_path_buf),
    };
    run_cached_buffer_conversion(cache, &bytes, &stream, cancel, image_dir.is_none(), convert)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::{File, FileTimes},
        sync::Arc,
    };

    fn cache(root: &Path) -> DocumentCache {
        DocumentCache::new(root.into(), CONVERTER_VERSION.into())
    }
    fn set_mtime(path: &Path, time: SystemTime) {
        File::options().write(true).open(path).unwrap().set_times(FileTimes::new().set_modified(time)).unwrap();
    }

    #[test]
    fn original_markit_cache_05_sweeps_orphaned_tmp_files_during_prune() {
        let dir = tempfile::tempdir().unwrap();
        let stale = dir.path().join("orphan.123.456.tmp");
        let fresh = dir.path().join("active.789.012.tmp");
        fs::write(&stale, "stale").unwrap();
        fs::write(&fresh, "fresh").unwrap();
        set_mtime(&stale, SystemTime::now() - Duration::from_secs(60 * 60));
        cache(dir.path()).prune();
        assert!(!stale.exists());
        assert!(fresh.exists());
    }

    #[test]
    fn cache_key_schema_version_extension_and_bytes_contract() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache(dir.path());
        let digest = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        assert_eq!(cache.key("\u{feff} ..PDF\u{00a0}", b"hello"), format!("v1-18_1_8-pdf-{digest}"));
        assert_eq!(cache.key("...", b"hello"), format!("v1-18_1_8-bin-{digest}"));
        assert_eq!(cache.key("application/PDF", b"hello"), format!("v1-18_1_8-application_pdf-{digest}"));
        assert_eq!(
            DocumentCache::new(dir.path().into(), "V2++3".into()).key("", b"hello"),
            format!("v1-V2_3-bin-{digest}")
        );
        assert_ne!(cache.key("pdf", b"hello"), cache.key("pdf", b"changed"));
    }

    #[test]
    fn cache_read_validity_cleanup_and_utf16_contract() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache(dir.path());
        let path = cache.entry_path("pdf", b"data");
        for bad in [
            "invalid",
            "null",
            "[]",
            r#"{"version":2,"content":"text"}"#,
            r#"{"version":1,"content":""}"#,
            r#"{"version":1,"content":12}"#,
        ] {
            fs::write(&path, bad).unwrap();
            assert!(cache.get("pdf", b"data").is_none());
            assert!(!path.exists());
        }
        fs::write(&path, r#"{"version":1.0,"content":"\ud800"}"#).unwrap();
        assert_eq!(cache.get_wire("pdf", b"data").unwrap().units(), &[0xd800]);
        let mtime = fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(cache.get("pdf", b"data").as_deref(), Some("\u{fffd}"));
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), mtime);
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(cache.get("pdf", b"data").is_none());
        assert!(path.is_dir()); // read failures are misses, not invalid-entry deletions
    }

    #[test]
    fn cache_atomic_concurrent_publish_and_nonfatal_failure() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(cache(dir.path()));
        let writers: Vec<_> = (0..16)
            .map(|i| {
                let cache = cache.clone();
                std::thread::spawn(move || cache.put("pdf", b"same bytes", &format!("body-{i}")))
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }
        assert!(cache.get(".pdf", b"same bytes").unwrap().starts_with("body-"));
        assert!(
            fs::read_dir(dir.path()).unwrap().all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp"))
        );
        let blocked = dir.path().join("blocked");
        fs::write(&blocked, "file").unwrap();
        DocumentCache::new(blocked.clone(), CONVERTER_VERSION.into()).put("pdf", b"data", "text");
        assert_eq!(fs::read_to_string(blocked).unwrap(), "file");
        cache.put("pdf", b"empty", ""); // fixed put writes empty; fixed get invalidates it
        assert!(cache.get("pdf", b"empty").is_none());
        assert!(!cache.entry_path("pdf", b"empty").exists());
    }

    #[test]
    fn cache_fifo_cap_all_json_no_read_touch_and_non_json_exclusion() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("v0-old-version.json");
        let recent = dir.path().join("unrelated.json");
        assert_eq!(MAX_CACHE_BYTES, 256 * 1024 * 1024);
        File::create(&old).unwrap().set_len(16).unwrap();
        File::create(&recent).unwrap().set_len(1).unwrap();
        set_mtime(&old, SystemTime::now() - Duration::from_secs(60));
        let other = dir.path().join("ignored.bin");
        File::create(&other).unwrap().set_len(16).unwrap();
        let tmp = dir.path().join("fresh.tmp");
        File::create(&tmp).unwrap().set_len(16).unwrap();
        let directory = dir.path().join("directory.json");
        fs::create_dir(&directory).unwrap();
        prune_root_with_max(dir.path(), 16);
        assert!(!old.exists());
        assert!(recent.exists());
        assert!(other.exists());
        assert!(tmp.exists());
        assert!(directory.is_dir());
        prune_root_with_max(dir.path(), 16);
        assert!(recent.exists());
        let missing = DocumentCache::new(dir.path().join("missing"), CONVERTER_VERSION.into());
        missing.prune();
        assert!(missing.get("pdf", b"data").is_none());
    }
}
