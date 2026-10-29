//! Fixed OMP596f2da ASAR read-for-rewrite and encoder.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;
use super::read_facade::{ReadLimits, legacy_limits::Policy};
use ara_rpc::WireValue;
fn invalid(message: impl AsRef<str>) -> ToolError {
    error(format!("Invalid ASAR archive: {}", message.as_ref()))
}
fn u32le(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn safe_number(value: Option<&WireValue>) -> Option<u64> {
    value
        .and_then(WireValue::as_number)
        .filter(|n| n.is_finite() && *n >= 0.0 && n.fract() == 0.0 && *n <= JS_SAFE as f64)
        .map(|n| n as u64)
}
fn bool_field(node: &WireValue, key: &str, label: &str) -> Result<Option<bool>> {
    match node.get(key) {
        None => Ok(None),
        Some(WireValue::Bool(b)) => Ok(Some(*b)),
        _ => Err(invalid(format!("entry '{label}' has a non-boolean {key} flag"))),
    }
}
fn hash(value: &WireValue) -> Option<String> {
    value
        .as_string()?
        .to_utf8()
        .ok()
        .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(|s| s.to_ascii_lowercase())
}
fn js_string(value: Option<&WireValue>) -> Result<String> {
    let Some(value) = value else {
        return Ok("undefined".into());
    };
    Ok(match value {
        WireValue::String(value) => utf8(value),
        WireValue::Object(_) if value.get("toString").is_some() => {
            // JSON cannot contain a callable replacement; OrdinaryToPrimitive
            // cannot obtain a primitive from the remaining Object.valueOf.
            return Err(error("Failed to read ASAR archive: Cannot convert object to primitive value"));
        }
        WireValue::Object(_) => "[object Object]".into(),
        WireValue::Number(value) if value.is_infinite() => {
            if value.is_sign_negative() {
                "-Infinity".into()
            } else {
                "Infinity".into()
            }
        }
        WireValue::Array(_) => {
            enum Piece<'a> {
                Value(&'a WireValue),
                Comma,
            }
            let mut work = vec![Piece::Value(value)];
            let mut output = String::new();
            while let Some(piece) = work.pop() {
                match piece {
                    Piece::Comma => output.push(','),
                    Piece::Value(WireValue::Array(items)) => {
                        for (i, item) in items.iter().enumerate().rev() {
                            work.push(Piece::Value(item));
                            if i > 0 {
                                work.push(Piece::Comma);
                            }
                        }
                    }
                    Piece::Value(WireValue::Null) => {}
                    Piece::Value(value) => output.push_str(&js_string(Some(value))?),
                }
            }
            output
        }
        _ => value.stringify(),
    })
}
fn integrity(node: &WireValue, path: &WireString, size: u64) -> Result<Option<String>> {
    let Some(value) = node.get("integrity") else {
        return Ok(None);
    };
    let label = shown(path);
    if !value.is_object() {
        return Err(invalid(format!("file '{label}' has an invalid integrity record")));
    }
    if !value.get("algorithm").and_then(WireValue::as_string).is_some_and(|s| s.equals_ascii("SHA256")) {
        return Err(invalid(format!(
            "file '{label}' uses unsupported integrity algorithm '{}'",
            js_string(value.get("algorithm"))?
        )));
    }
    let hash = hash(value.get("hash").unwrap_or(&WireValue::Null))
        .ok_or_else(|| invalid(format!("file '{label}' has an invalid integrity hash")))?;
    let block_size = safe_number(value.get("blockSize"))
        .filter(|n| *n > 0)
        .ok_or_else(|| invalid(format!("file '{label}' has an invalid integrity block size")))?;
    let blocks = value
        .get("blocks")
        .and_then(WireValue::as_array)
        .ok_or_else(|| invalid(format!("file '{label}' has invalid integrity blocks")))?;
    let expected = size.div_ceil(block_size).max(1);
    if blocks.len() as u64 != expected {
        return Err(invalid(format!("file '{label}' has an inconsistent integrity block count")));
    }
    for block in blocks {
        if self::hash(block).is_none() {
            return Err(invalid(format!("file '{label}' has an invalid integrity block hash")));
        }
    }
    Ok(Some(hash))
}
pub(super) fn read(source: &Source, archive_path: &Path) -> Result<Index> {
    read_impl(source, Some(archive_path), Policy::default())
}
pub(super) fn read_for_read(source: &Source, archive_path: Option<&Path>, limits: ReadLimits) -> Result<Index> {
    read_impl(source, archive_path, Policy(Some(limits)))
}
fn read_impl(source: &Source, archive_path: Option<&Path>, policy: Policy) -> Result<Index> {
    if source.size() < 16 || source.size() > JS_SAFE {
        return Err(invalid("truncated header"));
    }
    let prefix = read_range(source, 0, 8, policy)?;
    if policy.0.is_some() && prefix.len() != 8 {
        return Err(invalid("truncated size pickle"));
    }
    if u32le(&prefix, 0) != 4 {
        return Err(invalid("invalid size pickle"));
    }
    let header_size = u32le(&prefix, 4) as u64;
    if header_size < 8 {
        return Err(invalid("header is too small"));
    }
    policy.index(header_size, "ASAR header")?;
    let data_offset = checked_add(8, header_size, "ASAR header")?;
    if data_offset > source.size() {
        return Err(invalid("header extends beyond archive boundary"));
    }
    let header = read_range(source, 8, header_size, policy)?;
    if policy.0.is_some() && header.len() as u64 != header_size {
        return Err(invalid("truncated header pickle"));
    }
    let payload = u32le(&header, 0) as u64;
    let json_size = u32le(&header, 4) as u64;
    let padded = (json_size + 3) & !3;
    if payload + 4 != header_size || payload != 4 + padded {
        return Err(invalid("inconsistent pickle lengths"));
    }
    let json_end = (8 + json_size) as usize;
    if header[json_end..].iter().any(|&b| b != 0) {
        return Err(invalid("non-zero pickle string padding"));
    }
    let text = std::str::from_utf8(&header[8..json_end]).map_err(|_| invalid("header is not valid UTF-8"))?;
    // TextDecoder's default BOM behavior precedes the exact iterative JSON parse.
    let header = WireValue::parse(text.strip_prefix('\u{feff}').unwrap_or(text))
        .map_err(|_| invalid("header is not valid JSON"))?;
    let files =
        header.get("files").filter(|v| v.is_object()).ok_or_else(|| invalid("root must contain a files object"))?;
    let mut work: Vec<(WireString, &WireString, &WireValue)> = Vec::new();
    let mut scheduled = 0usize;
    let mut index = Index::default();
    index.read_limits = policy.0;
    for (name, node) in files.entries().unwrap() {
        scheduled += 1;
        policy.count(scheduled)?;
        work.push(("".into(), name, node));
    }
    while let Some((parent, name, node)) = work.pop() {
        policy.path_string(name, "entry name")?;
        if name.is_empty()
            || name.equals_ascii(".")
            || name.equals_ascii("..")
            || name.units().iter().any(|u| matches!(u, 0 | 47 | 92))
        {
            return Err(invalid(format!("invalid entry name '{}'", shown(name))));
        }
        if !node.is_object() {
            return Err(invalid(format!("entry '{}' must be an object", shown(name))));
        }
        let path = join(&parent, name);
        policy.path_string(&path, "member path")?;
        if normalize(&path, false).as_ref() != Some(&path) {
            return Err(invalid(format!("invalid entry path '{}'", shown(&path))));
        }
        let label = shown(&path);
        let unpacked = bool_field(node, "unpacked", &label)?;
        if let Some(link) = node.get("link") {
            let target = link
                .as_string()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| invalid(format!("entry '{label}' has an invalid link target")))?;
            policy.path_string(target, "link target")?;
            let target = portable(target);
            let target = if starts_slash(&target) || drive(&target, true) { None } else { normalize(&target, true) }
                .ok_or_else(|| invalid(format!("entry '{label}' links outside the archive")))?;
            index.upsert(Entry {
                path,
                directory: false,
                size: 0,
                mode: None,
                mtime: None,
                storage: Some(Storage::Link { target, resolve: true }),
            })?;
            continue;
        }
        if let Some(files) = node.get("files") {
            if !files.is_object() {
                return Err(invalid(format!("directory '{label}' must contain a files object")));
            }
            for (name, node) in files.entries().unwrap() {
                scheduled += 1;
                policy.count(scheduled)?;
                work.push((path.clone(), name, node));
            }
            index.upsert(Entry::directory(path))?;
            continue;
        }
        let size =
            safe_number(node.get("size")).ok_or_else(|| invalid(format!("file '{label}' has an invalid size")))?;
        policy.member(size, &path)?;
        let executable = match node.get("executable") {
            None => None,
            Some(WireValue::Bool(value)) => Some(*value),
            _ => {
                return Err(invalid(format!("file '{label}' has a non-boolean executable flag")));
            }
        };
        let mode = executable.map(|b| 0o100000 | if b { 0o755 } else { 0o644 });
        let integrity = integrity(node, &path, size)?;
        let storage = if unpacked == Some(true) {
            if policy.0.is_some() && archive_path.is_none() {
                Storage::ReadMember(Arc::new(ReadUnpacked { path: None, size, integrity }))
            } else {
            let archive_path = archive_path.expect("WRITE has a path; READ missing path remains deferred above");
            let mut unpacked = archive_path.as_os_str().to_owned();
            unpacked.push(".unpacked");
            #[cfg(windows)]
            let target = {
                use std::os::windows::ffi::{OsStrExt, OsStringExt};
                // Node win32.join concatenates admitted components before
                // normalization; C: inside metadata must not replace the root.
                // Entry admission above already excludes dot/slash components.
                let mut units: Vec<u16> = unpacked.encode_wide().collect();
                for part in path.units().split(|&u| u == 47) {
                    units.push(92);
                    units.extend_from_slice(part);
                }
                PathBuf::from(std::ffi::OsString::from_wide(&units))
            };
            #[cfg(not(windows))]
            let target = {
                let mut target = PathBuf::from(unpacked);
                for part in path.units().split(|&u| u == 47) {
                    target.push(utf8(&WireString::from_units(part.to_vec())));
                }
                target
            };
            if policy.0.is_some() {
                Storage::ReadMember(Arc::new(ReadUnpacked { path: Some(target), size, integrity }))
            } else {
                Storage::Unpacked { path: target, size, integrity }
            }
            }
        } else {
            let offset = node
                .get("offset")
                .and_then(WireValue::as_string)
                .and_then(|s| s.to_utf8().ok())
                .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
                .ok_or_else(|| invalid(format!("file '{label}' has an invalid offset")))?;
            let relative: f64 =
                offset.parse().map_err(|_| invalid(format!("file '{label}' extends beyond archive boundary")))?;
            if !relative.is_finite() || relative > JS_SAFE as f64 {
                return Err(invalid(format!("file '{label}' extends beyond archive boundary")));
            }
            let offset = data_offset + relative as u64;
            let end = offset + size;
            if offset > JS_SAFE || end > JS_SAFE || end > source.size() {
                return Err(invalid(format!("file '{label}' extends beyond archive boundary")));
            }
            if policy.0.is_some() {
                Storage::ReadMember(Arc::new(ReadPacked { source: source.clone(), offset, size, integrity }))
            } else {
                Storage::Slice { source: source.clone(), offset, size, integrity }
            }
        };
        index.upsert(Entry { path, directory: false, size, mode, mtime: None, storage: Some(storage) })?;
    }
    Ok(index)
}
fn read_range(source: &Source, offset: u64, size: u64, policy: Policy) -> Result<Vec<u8>> {
    if policy.0.is_none() { return source.range(offset, size); }
    super::read_facade::read_with_source_error(source, offset, size,
        |err| error(format!("Failed to read ASAR archive: {}", err.0)))
}
struct ReadPacked { source: Source, offset: u64, size: u64, integrity: Option<String> }
impl super::read_facade::ReadMemberSource for ReadPacked {
    fn read(&self, size: u64, path: &WireString) -> Result<Vec<u8>> {
        if size != self.size { return Err(error(format!("ASAR member '{}' has an inconsistent size", shown(path)))); }
        let bytes = super::read_facade::read_with_source_error(&self.source, self.offset, self.size,
            |err| error(format!("Failed to read ASAR member '{}': {}", shown(path), err.0)))?;
        if bytes.len() as u64 != self.size { return Err(error(format!("ASAR member '{}' is truncated", shown(path)))); }
        verify_integrity(&bytes, self.integrity.as_deref(), path)?;
        Ok(bytes)
    }
}
struct ReadUnpacked { path: Option<PathBuf>, size: u64, integrity: Option<String> }
impl super::read_facade::ReadMemberSource for ReadUnpacked {
    fn read(&self, size: u64, member_path: &WireString) -> Result<Vec<u8>> {
        let label = shown(member_path);
        if size != self.size { return Err(error(format!("ASAR member '{label}' has an inconsistent size"))); }
        let path = self.path.as_ref().ok_or_else(|| error(format!("Archive file '{label}' is unpacked and requires a filesystem-backed ASAR archive")))?;
        let stat = std::fs::metadata(path).map_err(|_| error(format!("Unpacked ASAR file '{label}' was not found")))?;
        if stat.is_dir() { return Err(error(format!("Unpacked ASAR file '{label}' is a directory"))); }
        if stat.len() != self.size { return Err(error(format!("Unpacked ASAR file '{label}' size differs from its archive header ({} != {} bytes)", stat.len(), self.size))); }
        let bytes = std::fs::read(path).map_err(|err| error(format!("Failed to read unpacked ASAR file '{label}': {err}")))?;
        if bytes.len() as u64 != self.size { return Err(error(format!("Unpacked ASAR file '{label}' changed while being read"))); }
        verify_integrity(&bytes, self.integrity.as_deref(), member_path)?;
        Ok(bytes)
    }
}
fn writer_path(raw: &WireString) -> Result<WireString> {
    let p = portable(raw);
    let normalized = normalize(&p, false);
    if normalized.is_none()
        || starts_slash(&p)
        || drive(&p, true)
        || p.units().contains(&0)
        || p.units().split(|&u| u == 47).any(|p| p.is_empty() || p == [46] || p == [46, 46])
    {
        return Err(error(format!("Invalid ASAR member path '{}'", shown(raw))));
    }
    Ok(normalized.unwrap())
}
enum TreeNode {
    Directory(usize),
    File { size: u64, offset: u64 },
}
#[derive(Default)]
struct TreeDirectory {
    entries: Vec<(WireString, TreeNode)>,
    by_name: HashMap<WireString, usize>,
}
impl TreeDirectory {
    fn get(&self, key: &WireString) -> Option<&TreeNode> {
        self.by_name.get(key).map(|&slot| &self.entries[slot].1)
    }
    fn insert(&mut self, key: WireString, node: TreeNode) {
        self.by_name.insert(key.clone(), self.entries.len());
        self.entries.push((key, node));
    }
}
pub(super) fn encode(members: &[(WireString, Vec<u8>)]) -> Result<Vec<u8>> {
    // Arena assembly and WireValue's iterative stringify avoid a recursion
    // limit in valid deep ASAR trees. WireString keys never roundtrip through UTF8.
    let mut dirs = vec![TreeDirectory::default()];
    let mut payload_size = 0u64;
    for (raw, bytes) in members {
        let path = writer_path(raw)?;
        let parts = path.units().split(|&u| u == 47).collect::<Vec<_>>();
        let mut directory = 0usize;
        for (depth, part) in parts[..parts.len() - 1].iter().enumerate() {
            let key = WireString::from_units(part.to_vec());
            match dirs[directory].get(&key) {
                Some(TreeNode::Directory(next)) => directory = *next,
                Some(TreeNode::File { .. }) => {
                    let ancestor = WireString::from_units(parts[..=depth].join(&47));
                    return Err(error(format!(
                        "ASAR member path '{}' crosses file '{}'",
                        utf8(&path),
                        utf8(&ancestor)
                    )));
                }
                None => {
                    let next = dirs.len();
                    dirs.push(TreeDirectory::default());
                    dirs[directory].insert(key, TreeNode::Directory(next));
                    directory = next;
                }
            }
        }
        let name = WireString::from_units(parts.last().unwrap().to_vec());
        if dirs[directory].get(&name).is_some() {
            return Err(error(format!("Duplicate or conflicting ASAR member path '{}'", utf8(&path))));
        }
        let size = bytes.len() as u64;
        let next = checked_add(payload_size, size, "ASAR payload")
            .map_err(|_| error("ASAR payload is too large to encode safely"))?;
        dirs[directory].insert(name, TreeNode::File { size, offset: payload_size });
        payload_size = next;
    }
    let mut built: Vec<Option<WireValue>> = (0..dirs.len()).map(|_| None).collect();
    for i in (0..dirs.len()).rev() {
        let mut files = Vec::new();
        for (name, node) in std::mem::take(&mut dirs[i].entries) {
            let value = match node {
                TreeNode::Directory(child) => built[child].take().unwrap(),
                TreeNode::File { size, offset } => WireValue::object(vec![
                    ("size", WireValue::Number(size as f64)),
                    ("offset", WireValue::String(offset.to_string().into())),
                ]),
            };
            files.push((name, value));
        }
        built[i] = Some(WireValue::object(vec![("files", WireValue::Object(files))]));
    }
    let json = built[0].as_ref().unwrap().stringify().into_bytes();
    let padded = (json.len() as u64 + 3) & !3;
    let inner = 4 + padded;
    let header = 4 + inner;
    if json.len() as u64 > u32::MAX as u64 || header > u32::MAX as u64 {
        return Err(error("ASAR header is too large to encode"));
    }
    let offset = 8 + header;
    let size = checked_add(offset, payload_size, "ASAR output")
        .map_err(|_| error("ASAR archive is too large to encode safely"))?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(usize::try_from(size).map_err(|_| error("ASAR archive is too large to encode in memory"))?)
        .map_err(|_| error("ASAR archive is too large to encode in memory"))?;
    output.resize(offset as usize, 0);
    output[..4].copy_from_slice(&4u32.to_le_bytes());
    output[4..8].copy_from_slice(&(header as u32).to_le_bytes());
    output[8..12].copy_from_slice(&(inner as u32).to_le_bytes());
    output[12..16].copy_from_slice(&(json.len() as u32).to_le_bytes());
    output[16..16 + json.len()].copy_from_slice(&json);
    for (_, bytes) in members {
        output.extend_from_slice(bytes);
    }
    Ok(output)
}
