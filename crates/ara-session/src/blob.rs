//! Lossless fixed-OMP image persistence (`session/blob-store.ts`,
//! `session-persistence.ts`, `session-loader.ts`, 596f2da; MIT).
//! The Host supplies the directory. Ordinary raw text and signed provider
//! state retain ARA's existing verbatim audit contract.
use base64::{
    Engine as _, alphabet,
    engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig, general_purpose::STANDARD},
};
use ring::digest::{SHA256, digest};
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub const BLOB_PREFIX: &str = "blob:sha256:";
pub const BLOB_EXTERNALIZE_THRESHOLD: usize = 1024;
const TRUNCATION_NOTICE: &str = "\n\n[Session persistence truncated large content]";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlobWarning {
    MalformedReference,
    MissingBlob { hash: String },
}

#[derive(Clone, Debug)]
pub struct BlobStore {
    directory: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobPutResult {
    pub hash: String,
    pub path: PathBuf,
    pub display_path: PathBuf,
    pub reference: String,
}

/// This is the sole path construction gate for persisted references.
pub fn parse_blob_ref(data: &str) -> Option<&str> {
    let hash = data.strip_prefix(BLOB_PREFIX)?;
    (hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
        .then_some(hash)
}

fn normalized_extension(extension: &str) -> Option<String> {
    let extension = extension.strip_prefix('.').unwrap_or(extension);
    (!extension.is_empty()
        && extension.len() <= 32
        && extension.as_bytes()[0].is_ascii_alphanumeric()
        && extension.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte)))
    .then(|| extension.to_ascii_lowercase())
}

fn image_extension(mime: Option<&str>) -> Option<String> {
    let mime = mime?.to_ascii_lowercase();
    let extension = match mime.as_str() {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        _ => mime.strip_prefix("image/")?.split(';').next()?.split('+').next()?,
    };
    normalized_extension(extension)
}

impl BlobStore {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self { directory: directory.into() }
    }
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Publish complete bytes before a journal can publish their reference.
    /// Existing content-addressed files are reused across Session rewrites.
    pub fn put(&self, bytes: &[u8], extension: Option<&str>) -> io::Result<BlobPutResult> {
        let hash = digest(&SHA256, bytes).as_ref().iter().map(|byte| format!("{byte:02x}")).collect::<String>();
        fs::create_dir_all(&self.directory)?;
        let path = self.directory.join(&hash);
        let exists = match fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() => true,
            Ok(_) => return Err(io::Error::new(io::ErrorKind::InvalidData, "canonical blob path is not a file")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(error),
        };
        if !exists {
            let temporary = self.directory.join(format!(".blob-{}", uuid::Uuid::new_v4().simple()));
            let published = (|| -> io::Result<()> {
                let mut file = OpenOptions::new().create_new(true).write(true).open(&temporary)?;
                file.write_all(bytes)?;
                file.sync_all()?;
                fs::rename(&temporary, &path)
            })();
            if let Err(error) = published {
                let _ = fs::remove_file(&temporary);
                // A concurrent writer may already have published these bytes.
                if !fs::metadata(&path).is_ok_and(|metadata| metadata.is_file()) {
                    return Err(error);
                }
            }
            crate::sync_dir(&self.directory)?;
        }
        let display_path = extension
            .and_then(normalized_extension)
            .map(|extension| self.directory.join(format!("{hash}.{extension}")))
            .unwrap_or_else(|| path.clone());
        if display_path != path && !display_path.exists() {
            if let Err(error) = fs::hard_link(&path, &display_path)
                && error.kind() != io::ErrorKind::AlreadyExists
            {
                let mut file = OpenOptions::new().write(true).create(true).truncate(true).open(&display_path)?;
                file.write_all(bytes)?;
                file.sync_all()?;
            }
            crate::sync_dir(&self.directory)?;
        }
        Ok(BlobPutResult { reference: format!("{BLOB_PREFIX}{hash}"), hash, path, display_path })
    }

    pub fn get(&self, hash: &str) -> io::Result<Option<Vec<u8>>> {
        if parse_blob_ref(&format!("{BLOB_PREFIX}{hash}")).is_none() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid blob hash"));
        }
        match fs::read(self.directory.join(hash)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn encode_image(&self, data: &str, mime: Option<&str>) -> io::Result<String> {
        if data.starts_with(BLOB_PREFIX) {
            return Ok(data.to_owned());
        }
        // Fixed Buffer.from(data, "base64") accepts URL-safe/unpadded input,
        // ignores whitespace/junk and stops at padding. Reopening canonicalizes
        // to standard padded base64 like Buffer.toString("base64").
        let mut cleaned = data
            .bytes()
            .take_while(|byte| *byte != b'=')
            .filter_map(|byte| match byte {
                b'-' => Some(b'+'),
                b'_' => Some(b'/'),
                byte if byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/') => Some(byte),
                _ => None,
            })
            .collect::<Vec<_>>();
        if cleaned.len() % 4 == 1 {
            cleaned.pop();
        }
        let decoder = GeneralPurpose::new(
            &alphabet::STANDARD,
            GeneralPurposeConfig::new()
                .with_decode_padding_mode(DecodePaddingMode::Indifferent)
                .with_decode_allow_trailing_bits(true),
        );
        let bytes =
            decoder.decode(cleaned).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid image base64"))?;
        Ok(self.put(&bytes, image_extension(mime).as_deref())?.reference)
    }

    fn resolve(&self, data: &str, url: bool, warnings: &mut Vec<BlobWarning>) -> io::Result<String> {
        if !data.starts_with(BLOB_PREFIX) {
            return Ok(data.to_owned());
        }
        let Some(hash) = parse_blob_ref(data) else {
            warnings.push(BlobWarning::MalformedReference);
            return Ok(data.to_owned());
        };
        let Some(bytes) = self.get(hash)? else {
            warnings.push(BlobWarning::MissingBlob { hash: hash.to_owned() });
            return Ok(data.to_owned());
        };
        if url { Ok(String::from_utf8_lossy(&bytes).into_owned()) } else { Ok(STANDARD.encode(bytes)) }
    }
}

fn image_position(value: &Value, key: Option<&str>) -> bool {
    if value.get("data").and_then(Value::as_str).is_none() {
        return false;
    }
    let image = value.get("type").and_then(Value::as_str) == Some("image");
    let image_mime = value
        .get("mimeType")
        .and_then(Value::as_str)
        .is_some_and(|mime| mime.to_ascii_lowercase().starts_with("image/"));
    (image || image_mime)
        && match key {
            Some("content") => image,
            Some("images" | "frames") => true,
            _ => false,
        }
}

fn atomic_provider_block(value: &Value) -> bool {
    let nonempty = |key| value.get(key).and_then(Value::as_str).is_some_and(|text| !text.is_empty());
    match value.get("type").and_then(Value::as_str) {
        Some("thinking") => nonempty("thinkingSignature"),
        Some("text") => nonempty("textSignature"),
        Some("toolCall") => nonempty("thoughtSignature"),
        Some("redactedThinking") => nonempty("data"),
        Some("reasoning") => nonempty("encrypted_content"),
        Some("anthropicServerTool") => true,
        _ => false,
    }
}

pub(crate) fn prepare_entry(entry: &Value, store: &BlobStore) -> io::Result<Value> {
    let mut prepared = entry.clone();
    externalize(&mut prepared, store, None)?;
    Ok(prepared)
}

fn externalize(value: &mut Value, store: &BlobStore, key: Option<&str>) -> io::Result<()> {
    if value.get("type").and_then(Value::as_str) == Some("image_generation_call")
        && let Some(data) = value
            .get("result")
            .and_then(Value::as_str)
            .filter(|data| !data.starts_with(BLOB_PREFIX) && data.encode_utf16().count() >= BLOB_EXTERNALIZE_THRESHOLD)
    {
        value["result"] = Value::String(store.encode_image(data, None)?);
        return Ok(());
    }
    if image_position(value, key) {
        let data = value["data"].as_str().expect("image position has data");
        if !data.starts_with(BLOB_PREFIX) && data.encode_utf16().count() >= BLOB_EXTERNALIZE_THRESHOLD {
            value["data"] = Value::String(store.encode_image(data, value.get("mimeType").and_then(Value::as_str))?);
        }
        return Ok(());
    }
    if atomic_provider_block(value) {
        return Ok(());
    }
    match value {
        Value::String(data)
            if key == Some("image_url") && data.starts_with("data:image/") && data.contains(";base64,") =>
        {
            *data = store.put(data.as_bytes(), None)?.reference;
        }
        Value::Array(values) => {
            for value in values {
                externalize(value, store, key)?;
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                externalize(value, store, Some(key))?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn resolve_entry(entry: &mut Value, store: &BlobStore, warnings: &mut Vec<BlobWarning>) -> io::Result<()> {
    resolve_tree(entry, store, warnings, None)
}

fn resolve_tree(
    value: &mut Value,
    store: &BlobStore,
    warnings: &mut Vec<BlobWarning>,
    key: Option<&str>,
) -> io::Result<()> {
    if image_position(value, key) {
        value["data"] = Value::String(store.resolve(value["data"].as_str().expect("image data"), false, warnings)?);
        return Ok(());
    }
    if value.get("type").and_then(Value::as_str) == Some("image_generation_call")
        && let Some(data) = value.get("result").and_then(Value::as_str)
    {
        value["result"] = Value::String(store.resolve(data, false, warnings)?);
    }
    match value {
        Value::String(data) if key == Some("image_url") => *data = store.resolve(data, true, warnings)?,
        Value::Array(values) => {
            for value in values {
                resolve_tree(value, store, warnings, key)?;
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                resolve_tree(value, store, warnings, Some(key))?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// The fixed loader converts already truncated legacy frames to retained source
/// text, or removes only broken frames when no complete source exists.
pub(crate) fn repair_truncated_archive(entry: &mut Value) {
    if entry.get("type").and_then(Value::as_str) != Some("compaction") {
        return;
    }
    let Some(archive) = ara_snapcompact::get_preserved_archive(entry.get("preserveData")) else {
        return;
    };
    if !archive.frames.iter().any(|frame| frame.data.ends_with(TRUNCATION_NOTICE)) {
        return;
    }
    let Some(slot) =
        entry.get_mut("preserveData").and_then(|data| data.get_mut("snapcompact")).and_then(Value::as_object_mut)
    else {
        return;
    };
    if let Some(text) = archive.text.filter(|text| !text.is_empty()) {
        slot.insert("frames".into(), Value::Array(vec![]));
        slot.insert("textHead".into(), Value::String(text));
        slot.insert("textTail".into(), Value::String(String::new()));
    } else {
        let frames =
            archive.frames.into_iter().filter(|frame| !frame.data.ends_with(TRUNCATION_NOTICE)).collect::<Vec<_>>();
        slot.insert("frames".into(), serde_json::to_value(frames).expect("archive frames serialize"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn persistence_image_families_roundtrip_without_changing_raw_or_signed_fields() {
        let directory = tempfile::tempdir().unwrap();
        let store = BlobStore::new(directory.path());
        let bytes = vec![24u8; 400_000];
        let data = STANDARD.encode(&bytes);
        let url = format!("data:image/png;base64,{data}");
        let signed_text = "signed raw content".repeat(40_000);
        let entry = json!({"type":"message","message":{
            "content":[{"type":"image","data":data,"mimeType":"image/png","detail":"original"},
                {"type":"text","text":signed_text,"textSignature":"signature"},
                {"type":"thinking","thinking":"thinking","thinkingSignature":"signature"},
                {"type":"toolCall","arguments":{"text":signed_text},"thoughtSignature":"signature"},
                {"type":"redactedThinking","data":signed_text},
                {"type":"anthropicServerTool","block":{"type":"future","data":signed_text}}],
            "providerPayload":{"items":[{"type":"image_generation_call","result":data},
                {"type":"message","content":[{"image_url":url}]},
                {"type":"reasoning","encrypted_content":signed_text}]},
            "details":{"content":signed_text,"jsonlEvents":["retained raw event"]}},
            "images":[{"data":data,"mimeType":"image/jpeg"}],
            "preserveData":{"snapcompact":{"frames":[{"data":data,"mimeType":"image/png","chars":1,"cols":1,"rows":1}],
                "text":signed_text}}});
        let original = entry.clone();
        let mut persisted = prepare_entry(&entry, &store).unwrap();
        assert_eq!(entry, original, "disk preparation must not change the live source");
        for value in [
            &persisted["message"]["content"][0]["data"],
            &persisted["images"][0]["data"],
            &persisted["preserveData"]["snapcompact"]["frames"][0]["data"],
            &persisted["message"]["providerPayload"]["items"][0]["result"],
        ] {
            let hash = parse_blob_ref(value.as_str().unwrap()).unwrap();
            assert_eq!(store.get(hash).unwrap().unwrap(), bytes);
        }
        let hash = parse_blob_ref(
            persisted["message"]["providerPayload"]["items"][1]["content"][0]["image_url"].as_str().unwrap(),
        )
        .unwrap();
        assert_eq!(store.get(hash).unwrap().unwrap(), url.as_bytes());
        let mut warnings = Vec::new();
        resolve_entry(&mut persisted, &store, &mut warnings).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(persisted, original);
        let result = store.put(&bytes, None).unwrap();
        fs::remove_file(&result.path).unwrap();
        fs::create_dir(&result.path).unwrap();
        assert!(prepare_entry(&entry, &store).is_err(), "a directory cannot be published as a readable blob reference");
        // Buffer's tolerant base64 input is canonicalized on disk/read.
        let alternative = "-_8 \n".repeat(300);
        let reference = store.encode_image(&alternative, Some("image/webp")).unwrap();
        assert_eq!(
            STANDARD.decode(store.resolve(&reference, false, &mut warnings).unwrap()).unwrap(),
            STANDARD.decode("+/8".repeat(300)).unwrap()
        );
        let blocked = directory.path().join("blocked");
        fs::write(&blocked, "file").unwrap();
        assert!(prepare_entry(&entry, &BlobStore::new(blocked)).is_err());
        assert_eq!(entry, original);
    }
}
