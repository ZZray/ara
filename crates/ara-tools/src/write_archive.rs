//! Fixed OMP596f2da archive WRITE and private read-for-rewrite.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
mod read_facade;
pub(crate) use read_facade::{Reader as ReadArchive, path_candidates as read_archive_path_candidates};

mod asar;
mod codecs;
mod tar;
mod zip;

use ara_agent::ToolError;
use ara_rpc::WireString;
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
type Result<T> = std::result::Result<T, ToolError>;
const MAX_ENTRIES: usize = 1_000_000;
const MAX_MEMORY: u64 = 256 * 1024 * 1024;
const MAX_INDEX: u64 = 64 * 1024 * 1024;
const MAX_MEMBER: u64 = 64 * 1024 * 1024;
const MAX_PATH: usize = 4096;
const MAX_LINKS: usize = 40;
const JS_SAFE: u64 = 9_007_199_254_740_991;

// Names stay in their ECMAScript domain until a format's UTF8 encoding boundary.
fn utf8(name: &WireString) -> String {
    String::from_utf16_lossy(name.units())
}
fn path_len(name: &WireString) -> usize {
    utf8(name).len()
}
fn shown(name: &WireString) -> String {
    let text = utf8(name);
    if text.len() <= 256 {
        return text;
    }
    let mut end = 253.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &text[..end])
}
fn error(message: impl Into<String>) -> ToolError {
    ToolError(message.into())
}
fn io_error(e: std::io::Error) -> ToolError {
    error(e.to_string())
}
fn checked_add(a: u64, b: u64, what: &str) -> Result<u64> {
    a.checked_add(b)
        .filter(|&n| n <= JS_SAFE)
        .ok_or_else(|| error(format!("Invalid archive: {what} has an invalid range")))
}
fn memory_size(n: u64) -> Result<()> {
    if n > JS_SAFE {
        return Err(error("Archive is too large to read safely"));
    }
    if n > MAX_MEMORY {
        return Err(error(format!(
            "Archive is too large to read in memory ({} > {} limit)",
            crate::format_bytes(n),
            crate::format_bytes(MAX_MEMORY)
        )));
    }
    Ok(())
}
fn index_size(n: u64, what: &str) -> Result<()> {
    if n > JS_SAFE {
        return Err(error(format!("Invalid archive: {what} has an invalid size")));
    }
    if n > MAX_INDEX {
        return Err(error(format!(
            "Archive {what} is too large ({} > {} limit)",
            crate::format_bytes(n),
            crate::format_bytes(MAX_INDEX)
        )));
    }
    Ok(())
}
fn member_size(n: u64, name: &WireString) -> Result<()> {
    if n > JS_SAFE {
        return Err(error(format!("Archive member '{}' has an invalid size", shown(name))));
    }
    if n > MAX_MEMBER {
        return Err(error(format!(
            "Archive member '{}' is too large to extract in memory ({} > {} limit)",
            shown(name),
            crate::format_bytes(n),
            crate::format_bytes(MAX_MEMBER)
        )));
    }
    Ok(())
}
fn count(n: usize) -> Result<()> {
    if n > MAX_ENTRIES { Err(error(format!("Archive has too many entries (> {MAX_ENTRIES} limit)"))) } else { Ok(()) }
}
fn path_bytes(n: usize, field: &str) -> Result<()> {
    if n > MAX_PATH { Err(error(format!("Archive {field} exceeds {MAX_PATH} bytes"))) } else { Ok(()) }
}
fn path_string(name: &WireString, field: &str) -> Result<()> {
    path_bytes(path_len(name), field)
}
fn portable(name: &WireString) -> WireString {
    WireString::from_units(name.units().iter().map(|&u| if u == 92 { 47 } else { u }).collect())
}
fn normalize(name: &WireString, allow_root: bool) -> Option<WireString> {
    let p = portable(name);
    let mut out = Vec::new();
    for part in p.units().split(|&u| u == 47) {
        if part.is_empty() || part == [46] {
            continue;
        }
        if part == [46, 46] {
            return None;
        }
        if !out.is_empty() {
            out.push(47);
        }
        out.extend_from_slice(part);
    }
    (allow_root || !out.is_empty()).then(|| WireString::from_units(out))
}
fn dirname(name: &WireString) -> WireString {
    WireString::from_units(
        name.units().iter().rposition(|&u| u == 47).map_or_else(Vec::new, |i| name.units()[..i].to_vec()),
    )
}
fn join(a: &WireString, b: &WireString) -> WireString {
    let mut out = a.units().to_vec();
    if !out.is_empty() && !b.is_empty() {
        out.push(47);
    }
    out.extend_from_slice(b.units());
    WireString::from_units(out)
}
// node:path.posix.join normalizes parent segments before archive lookup rejects
// a surviving traversal. Absolute target admission is format-specific.
fn posix_join(a: &WireString, b: &WireString) -> WireString {
    let joined = join(a, b);
    let mut parts: Vec<&[u16]> = Vec::new();
    for part in joined.units().split(|&u| u == 47) {
        if part.is_empty() || part == [46] {
            continue;
        }
        if part == [46, 46] && parts.last().is_some_and(|p| *p != [46, 46]) {
            parts.pop();
        } else {
            parts.push(part);
        }
    }
    let mut out = Vec::new();
    for p in parts {
        if !out.is_empty() {
            out.push(47);
        }
        out.extend_from_slice(p);
    }
    WireString::from_units(out)
}
fn drive(name: &WireString, slash_required: bool) -> bool {
    let u = name.units();
    u.len() >= 2
        && ((65..=90).contains(&u[0]) || (97..=122).contains(&u[0]))
        && u[1] == 58
        && (!slash_required || u.get(2) == Some(&47))
}
fn starts_slash(name: &WireString) -> bool {
    name.units().first() == Some(&47)
}

#[derive(Clone)]
enum Source {
    File { file: Arc<Mutex<File>>, size: u64 },
    Memory(Arc<Vec<u8>>),
    Read(Arc<dyn read_facade::ReadByteSource>),
}
impl Source {
    fn open(path: &Path) -> Result<Self> {
        let file = File::open(path).map_err(io_error)?;
        let size = file.metadata().map_err(io_error)?.len();
        Ok(Self::File { file: Arc::new(Mutex::new(file)), size })
    }
    fn size(&self) -> u64 {
        match self {
            Self::File { size, .. } => *size,
            Self::Memory(b) => b.len() as u64,
            Self::Read(source) => source.size(),
        }
    }
    fn range(&self, start: u64, len: u64) -> Result<Vec<u8>> {
        if let Self::Read(source) = self {
            read_facade::validate_range(start, len)?;
            return source.range(start, len);
        }
        let end = checked_add(start, len, "member data")?;
        if end > self.size() {
            return Err(error("Invalid archive: truncated data"));
        }
        let length = usize::try_from(len).map_err(|_| error("Archive is too large to read safely"))?;
        match self {
            Self::Memory(b) => Ok(b[start as usize..end as usize].to_vec()),
            Self::Read(_) => unreachable!("READ source already delegated"),
            Self::File { file, .. } => {
                let mut file = file.lock().unwrap_or_else(|e| e.into_inner());
                file.seek(SeekFrom::Start(start)).map_err(io_error)?;
                let mut b = Vec::new();
                b.try_reserve_exact(length).map_err(|_| error("Archive is too large to read in memory"))?;
                b.resize(length, 0);
                file.read_exact(&mut b).map_err(io_error)?;
                Ok(b)
            }
        }
    }
    fn all_bounded(&self) -> Result<Vec<u8>> {
        memory_size(self.size())?;
        self.range(0, self.size())
    }
}

#[derive(Clone)]
enum Storage {
    Slice { source: Source, offset: u64, size: u64, integrity: Option<String> },
    Unpacked { path: PathBuf, size: u64, integrity: Option<String> },
    Zip(zip::Member),
    ReadMember(Arc<dyn read_facade::ReadMemberSource>),
    Link { target: WireString, resolve: bool },
    Sparse,
}
#[derive(Clone)]
struct Entry {
    path: WireString,
    directory: bool,
    size: u64,
    mode: Option<u32>,
    mtime: Option<f64>,
    storage: Option<Storage>,
}
impl Entry {
    fn directory(path: WireString) -> Self {
        Self { path, directory: true, size: 0, mode: None, mtime: None, storage: None }
    }
}
#[derive(Default)]
struct Index {
    entries: Vec<Entry>,
    by_path: HashMap<WireString, usize>,
    read_limits: Option<read_facade::ReadLimits>,
    // READ raw metadata only; WRITE retains its unchanged u32 mode contract.
    read_mode_overrides: HashMap<WireString, u64>,
}
impl Index {
    fn read_mode(&self, path: &WireString, entry: &Entry) -> Option<f64> {
        self.read_mode_overrides.get(path).map(|&mode| mode as f64).or_else(|| entry.mode.map(f64::from))
    }
    fn count_entries(&self) -> Result<()> {
        match self.read_limits {
            Some(limits) => limits.entries(self.entries.len()),
            None => count(self.entries.len()),
        }
    }
    fn get(&self, path: &WireString) -> Option<&Entry> {
        self.by_path.get(path).map(|&i| &self.entries[i])
    }
    fn upsert(&mut self, mut entry: Entry) -> Result<Option<usize>> {
        if let Some(&i) = self.by_path.get(&entry.path) {
            let old = &self.entries[i];
            if !old.directory && entry.directory {
                return Ok(None);
            }
            if old.directory == entry.directory {
                entry.mode = entry.mode.or(old.mode);
                entry.mtime = entry.mtime.or(old.mtime);
                entry.storage = entry.storage.or_else(|| old.storage.clone());
            }
            self.entries[i] = entry;
            return Ok(Some(i));
        }
        let i = self.entries.len();
        self.by_path.insert(entry.path.clone(), i);
        self.entries.push(entry);
        self.count_entries()?;
        Ok(Some(i))
    }
    fn parents(&mut self) -> Result<()> {
        self.count_entries()?;
        let n = self.entries.len();
        for i in 0..n {
            let path = self.entries[i].path.clone();
            for end in path.units().iter().enumerate().filter_map(|(i, &u)| (u == 47).then_some(i)) {
                let parent = WireString::from_units(path.units()[..end].to_vec());
                if !parent.is_empty() && !self.by_path.contains_key(&parent) {
                    self.upsert(Entry::directory(parent))?;
                }
            }
        }
        Ok(())
    }
    fn resolve_link(&self, path: &WireString) -> Result<WireString> {
        let mut current = path.clone();
        let mut seen = HashSet::new();
        let mut rewrites = 0;
        while seen.insert(current.clone()) {
            let mut replacement = None;
            let mut end = current.len();
            while end > 0 {
                let prefix = WireString::from_units(current.units()[..end].to_vec());
                if let Some(entry) = self.get(&prefix) {
                    if let Some(Storage::Link { target, resolve }) = &entry.storage {
                        if entry.directory || *resolve {
                            let suffix = WireString::from_units(current.units().get(end + 1..).unwrap_or(&[]).to_vec());
                            replacement = Some(join(target, &suffix));
                            break;
                        }
                    }
                }
                end = current.units()[..end].iter().rposition(|&u| u == 47).unwrap_or(0);
            }
            let Some(next) = replacement else {
                return Ok(current);
            };
            rewrites += 1;
            if rewrites > self.read_limits.map_or(MAX_LINKS, |limits| limits.max_links) {
                break;
            }
            current = next;
        }
        Err(error(format!("Archive path '{}' crosses a cyclic symlink", utf8(path))))
    }
    fn read_file(&self, path: &WireString) -> Result<Vec<u8>> {
        let normalized =
            normalize(path, true).filter(|p| !p.is_empty()).ok_or_else(|| error("Archive file path is required"))?;
        let resolved = self.resolve_link(&normalized)?;
        if resolved.is_empty() {
            return Err(error(format!("Archive path '{}' is a directory", utf8(&normalized))));
        }
        let entry =
            self.get(&resolved).ok_or_else(|| error(format!("Archive file '{}' not found", utf8(&normalized))))?;
        if entry.directory {
            return Err(error(format!("Archive path '{}' is a directory", utf8(&normalized))));
        }
        let storage = entry
            .storage
            .as_ref()
            .ok_or_else(|| error(format!("Archive file '{}' has no readable storage", utf8(&normalized))))?;
        match self.read_limits {
            Some(limits) => limits.member(entry.size, &normalized)?,
            None => member_size(entry.size, &normalized)?,
        }
        match storage {
            Storage::Link { target, .. } => Err(error(format!(
                "Archive symlink '{}' cannot be materialized from target '{}'",
                shown(&normalized),
                shown(target)
            ))),
            Storage::Sparse => {
                Err(error(format!("Archive member '{}' is a sparse file and cannot be read", shown(&normalized))))
            }
            Storage::Zip(member) => member.read(entry.size, &normalized),
            Storage::ReadMember(member) => member.read(entry.size, &normalized),
            Storage::Slice { source, offset, size, integrity } => {
                if entry.size != *size {
                    return Err(error(format!("ASAR member '{}' has an inconsistent size", shown(&normalized))));
                }
                let bytes = source.range(*offset, *size)?;
                if matches!(source, Source::Read(_)) && bytes.len() as u64 != *size {
                    return Err(error(format!("Archive member '{}' is truncated", utf8(&normalized))));
                }
                verify_integrity(&bytes, integrity.as_deref(), &normalized)?;
                Ok(bytes)
            }
            Storage::Unpacked { path, size, integrity } => {
                let stat = std::fs::metadata(path)
                    .map_err(|_| error(format!("Unpacked ASAR file '{}' was not found", shown(&normalized))))?;
                if stat.is_dir() {
                    return Err(error(format!("Unpacked ASAR file '{}' is a directory", shown(&normalized))));
                }
                if stat.len() != *size {
                    return Err(error(format!(
                        "Unpacked ASAR file '{}' size differs from its archive header ({} != {} bytes)",
                        shown(&normalized),
                        stat.len(),
                        size
                    )));
                }
                let bytes = std::fs::read(path).map_err(io_error)?;
                if bytes.len() as u64 != *size {
                    return Err(error(format!("Unpacked ASAR file '{}' changed while being read", shown(&normalized))));
                }
                verify_integrity(&bytes, integrity.as_deref(), &normalized)?;
                Ok(bytes)
            }
        }
    }
    fn materialize(mut self) -> Result<Members> {
        self.parents()?;
        let mut members = Members::default();
        let mut total = 0u64;
        for entry in &self.entries {
            if entry.directory {
                continue;
            }
            let bytes = self.read_file(&entry.path)?;
            total = checked_add(total, bytes.len() as u64, "materialized archive")?;
            memory_size(total)?;
            members.set(entry.path.clone(), bytes);
        }
        Ok(members)
    }
}
fn verify_integrity(bytes: &[u8], expected: Option<&str>, path: &WireString) -> Result<()> {
    if let Some(expected) = expected {
        use sha2::Digest;
        let actual = format!("{:x}", sha2::Sha256::digest(bytes));
        if actual != expected {
            return Err(error(format!("ASAR member '{}' failed SHA256 integrity verification", shown(path))));
        }
    }
    Ok(())
}
#[derive(Default)]
struct Members {
    ordered: Vec<(WireString, Vec<u8>)>,
    by_path: HashMap<WireString, usize>,
}
impl Members {
    fn set(&mut self, path: WireString, bytes: Vec<u8>) {
        if let Some(&i) = self.by_path.get(&path) {
            self.ordered[i].1 = bytes;
        } else {
            self.by_path.insert(path.clone(), self.ordered.len());
            self.ordered.push((path, bytes));
        }
    }
    fn contains(&self, path: &WireString) -> bool {
        self.by_path.contains_key(path)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    Zip,
    Tar,
    TarGz,
    TarZst,
    Asar,
    Readonly(&'static str),
}
const EXTENSIONS: &[(&str, Format)] = &[
    ("tar.gz", Format::TarGz),
    ("tar.bz2", Format::Readonly("tar.bz2")),
    ("tar.xz", Format::Readonly("tar.xz")),
    ("tar.zst", Format::TarZst),
    ("tar.z", Format::Readonly("tar.Z")),
    ("zip", Format::Zip),
    ("jar", Format::Zip),
    ("war", Format::Zip),
    ("ear", Format::Zip),
    ("apk", Format::Zip),
    ("whl", Format::Zip),
    ("ipa", Format::Zip),
    ("xpi", Format::Zip),
    ("vsix", Format::Zip),
    ("nupkg", Format::Zip),
    ("cbz", Format::Zip),
    ("tar", Format::Tar),
    ("tgz", Format::TarGz),
    ("tbz2", Format::Readonly("tar.bz2")),
    ("tbz", Format::Readonly("tar.bz2")),
    ("txz", Format::Readonly("tar.xz")),
    ("tzst", Format::TarZst),
    ("asar", Format::Asar),
    ("rar", Format::Readonly("rar")),
    ("cbr", Format::Readonly("rar")),
    ("7z", Format::Readonly("7z")),
    ("iso", Format::Readonly("iso")),
    ("cab", Format::Readonly("cab")),
    ("cpio", Format::Readonly("cpio")),
    ("rpm", Format::Readonly("rpm")),
    ("ar", Format::Readonly("ar")),
    ("a", Format::Readonly("ar")),
    ("lib", Format::Readonly("ar")),
    ("deb", Format::Readonly("deb")),
    ("lzh", Format::Readonly("lzh")),
    ("lha", Format::Readonly("lzh")),
    ("arj", Format::Readonly("arj")),
    ("gz", Format::Readonly("gz")),
    ("bz2", Format::Readonly("bz2")),
    ("xz", Format::Readonly("xz")),
    ("zst", Format::Readonly("zst")),
    ("z", Format::Readonly("Z")),
    ("lzma", Format::Readonly("lzma")),
];
fn format_from_path(path: &Path) -> Option<Format> {
    let name = path.to_string_lossy().to_ascii_lowercase();
    EXTENSIONS
        .iter()
        .filter(|(ext, _)| name.ends_with(&format!(".{ext}")))
        .max_by_key(|(ext, _)| ext.len())
        .map(|(_, format)| *format)
}
fn candidates(authored: &str) -> Vec<(String, WireString)> {
    let lower = authored.to_ascii_lowercase().replace('\\', "/");
    let mut result = Vec::new();
    for (start, _) in lower.match_indices('.') {
        let longest = EXTENSIONS
            .iter()
            .filter_map(|(ext, _)| {
                let end = start + 1 + ext.len();
                (lower.get(start + 1..end) == Some(*ext)
                    && lower.get(end..).is_some_and(|rest| rest.is_empty() || rest.starts_with(':')))
                .then_some(end)
            })
            .max();
        if let Some(end) = longest {
            let outer = authored[..end].to_owned();
            if outer != authored {
                result.push((outer, lower[end..].trim_start_matches(':').into()));
                // Preserve member spelling/case, normalizing separators only.
                result.last_mut().unwrap().1 = authored[end..].replace('\\', "/").trim_start_matches(':').into();
            }
        }
    }
    result.sort_by_key(|(outer, _)| std::cmp::Reverse(outer.len()));
    result.dedup();
    result
}
fn normalize_write(path: &WireString) -> Result<WireString> {
    let p = portable(path);
    if p.is_empty() {
        return Err(error("Archive write path must target a file inside the archive"));
    }
    if p.units().last() == Some(&47) {
        return Err(error("Archive write path must target a file, not a directory"));
    }
    normalize(&p, true).ok_or_else(|| error("Archive path cannot contain '..'")).and_then(|p| {
        if p.is_empty() { Err(error("Archive write path must target a file inside the archive")) } else { Ok(p) }
    })
}
pub(super) struct Resolved {
    pub absolute: PathBuf,
    pub authored: String,
    member: WireString,
    pub exists: bool,
}
pub(super) fn resolve(
    authored: &str,
    mut resolve_path: impl FnMut(&str) -> Result<PathBuf>,
) -> Result<Option<Resolved>> {
    let candidates = candidates(authored);
    let Some((fallback, subpath)) = candidates.last() else {
        return Ok(None);
    };
    let fallback = Resolved {
        absolute: resolve_path(fallback)?,
        authored: fallback.clone(),
        member: normalize_write(subpath)?,
        exists: false,
    };
    for (outer, member) in candidates {
        let absolute = resolve_path(&outer)?;
        match std::fs::metadata(&absolute) {
            Ok(stat) if stat.is_dir() => continue,
            Ok(_) => {
                return Ok(Some(Resolved {
                    absolute,
                    authored: outer,
                    member: normalize_write(&member)?,
                    exists: true,
                }));
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory) => {}
            Err(e) => return Err(io_error(e)),
        }
    }
    Ok(Some(fallback))
}
impl Resolved {
    pub fn display_member(&self) -> String {
        utf8(&self.member)
    }
}
fn read_members(path: &Path, format: Format) -> Result<Members> {
    let source = Source::open(path)?;
    let index = match format {
        Format::Zip => zip::read(&source)?,
        Format::Asar => asar::read(&source, path)?,
        Format::Tar => tar::read(source.all_bounded()?)?,
        Format::TarGz | Format::TarZst => {
            let input = source.all_bounded()?;
            let bytes = if format == Format::TarGz {
                codecs::gunzip(&input, MAX_MEMORY)?
            } else {
                codecs::unzstd(&input, MAX_MEMORY)?
            };
            memory_size(bytes.len() as u64)?;
            if tar::sniff(&bytes) {
                tar::read(bytes)?
            } else {
                let base = path.file_name().unwrap_or_default().to_string_lossy();
                let lower = base.to_ascii_lowercase();
                let len = EXTENSIONS
                    .iter()
                    .filter(|(ext, _)| lower.len() > ext.len() + 1 && lower.ends_with(&format!(".{ext}")))
                    .max_by_key(|(ext, _)| ext.len())
                    .map(|(ext, _)| base.len() - ext.len() - 1);
                let name: WireString = len.map_or_else(|| base.to_string(), |len| base[..len].to_string()).into();
                let size = bytes.len() as u64;
                let mut index = Index::default();
                index.upsert(Entry {
                    path: name,
                    directory: false,
                    size,
                    mode: None,
                    mtime: None,
                    storage: Some(Storage::Slice {
                        source: Source::Memory(Arc::new(bytes)),
                        offset: 0,
                        size,
                        integrity: None,
                    }),
                })?;
                index
            }
        }
        Format::Readonly(name) => {
            return Err(error(format!("Writing entries inside {name} archives is not supported (read-only format).")));
        }
    };
    index.materialize()
}
fn encode(format: Format, members: &Members) -> Result<Vec<u8>> {
    match format {
        Format::Zip => zip::encode(&members.ordered),
        Format::Tar => tar::encode(&members.ordered),
        Format::Asar => asar::encode(&members.ordered),
        Format::TarGz => codecs::gzip(&tar::encode(&members.ordered)?),
        Format::TarZst => codecs::zstd(&tar::encode(&members.ordered)?),
        Format::Readonly(name) => {
            Err(error(format!("Writing entries inside {name} archives is not supported (read-only format).")))
        }
    }
}
pub(super) async fn rewrite(resolved: &Resolved, content: &str) -> Result<()> {
    let final_path = if resolved.exists {
        tokio::fs::canonicalize(&resolved.absolute).await.unwrap_or_else(|_| resolved.absolute.clone())
    } else {
        resolved.absolute.clone()
    };
    let format = format_from_path(&final_path).unwrap_or(Format::Tar);
    if let Format::Readonly(name) = format {
        return Err(error(format!("Writing entries inside {name} archives is not supported (read-only format).")));
    }
    if let Some(parent) = resolved.absolute.parent().filter(|p| !p.as_os_str().is_empty() && *p != Path::new(".")) {
        tokio::fs::create_dir_all(parent).await.map_err(io_error)?;
    }
    let mut members = if resolved.exists { read_members(&final_path, format)? } else { Members::default() };
    let target = format!("{}:{}", resolved.authored, utf8(&resolved.member));
    if content.is_empty() && !members.contains(&resolved.member) {
        if let Some(selector) = crate::paths::split_path_and_sel(&target).1 {
            return Err(error(format!(
                "write target '{target}' ends with a read-tool selector ':{selector}' and no such file exists — refusing to create a literal file by that name. If you meant to read it, use read({{ path: \"{target}\" }}). If you truly intend to create this file, pass its contents in `content` (a non-empty write is never blocked)."
            )));
        }
    }
    members.set(resolved.member.clone(), content.as_bytes().to_vec());
    let mut tmp = final_path.as_os_str().to_owned();
    tmp.push(format!(".tmp-{}", std::process::id()));
    let tmp = PathBuf::from(tmp);
    let result = async {
        let bytes = encode(format, &members)?;
        tokio::fs::write(&tmp, bytes).await.map_err(io_error)?;
        tokio::fs::rename(&tmp, &final_path).await.map_err(io_error)
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&tmp).await;
    }
    result
}
