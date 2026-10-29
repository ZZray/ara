//! Fixed OMP596f2da ar/deb.ts UnixAr and nested TAR composition.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;

fn first_member_name(bytes: &[u8]) -> Option<String> {
    if !bytes.starts_with(b"!<arch>\n") || bytes.len() < 68 {
        return None;
    }
    if bytes[66..68] != [0x60, 0x0a] {
        return None;
    }
    let mut name_end = 24;
    while name_end > 8 && bytes[name_end - 1] == b' ' {
        name_end -= 1;
    }
    let name = &bytes[8..name_end];
    if name.iter().any(|&byte| !(0x20..=0x7e).contains(&byte)) {
        return None;
    }
    let raw_name = std::str::from_utf8(name).ok()?;
    if let Some(length_text) = raw_name.strip_prefix("#1/") {
        if length_text.is_empty() || !length_text.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let length = length_text.parse::<u64>().ok()?;
        if length == 0 || length > JS_SAFE || length > bytes.len().saturating_sub(68) as u64 {
            return None;
        }
        let name_bytes = &bytes[68..68 + length as usize];
        let name_bytes = &name_bytes[..name_bytes.iter().position(|&b| b == 0).unwrap_or(name_bytes.len())];
        return Some(encoding_rs::UTF_8.decode_with_bom_removal(name_bytes).0.into_owned());
    }
    Some(raw_name.strip_suffix('/').unwrap_or(raw_name).to_owned())
}
pub(super) fn sniff(bytes: &[u8]) -> bool {
    first_member_name(bytes).as_deref() == Some("debian-binary")
}
fn classify(path: &WireString) -> Option<(bool, &'static str)> {
    let path = utf8(path);
    let (control, suffix) = if let Some(suffix) = path.strip_prefix("control.tar") {
        (true, suffix)
    } else if let Some(suffix) = path.strip_prefix("data.tar") {
        (false, suffix)
    } else {
        return None;
    };
    Some((
        control,
        match suffix {
            "" => "none",
            ".gz" => "gzip",
            ".xz" => "xz",
            ".zst" => "zstd",
            ".bz2" => "bzip2",
            ".lzma" => "lzma",
            _ => return None,
        },
    ))
}
fn prefix_control(mut entry: Entry) -> Entry {
    let control = WireString::from("control");
    entry.path = join(&control, &entry.path);
    if let Some(Storage::Link { target, .. }) = &mut entry.storage {
        if !target.is_empty() && normalize(target, false).as_ref() != Some(target) {
            return entry;
        }
        *target = if target.is_empty() { control } else { join(&control, target) };
    }
    entry
}
pub(super) fn read(
    source: Source,
    limits: &ReadLimits,
    decode: impl Fn(&str, &[u8], u64) -> Result<Vec<u8>>,
    read_tar: impl Fn(Vec<u8>, &ReadLimits) -> Result<Index>,
) -> Result<Index> {
    let probe_end = source.size().min(68 + limits.max_path as u64);
    let probe = source.range(0, probe_end)?;
    if !sniff(&probe) {
        return Err(error("Invalid deb archive: first member is not debian-binary"));
    }
    let outer = unix_ar::read(&source, limits)?;
    if outer.entries.first().map(|entry| utf8(&entry.path)).as_deref() != Some("debian-binary") {
        return Err(error("Invalid deb archive: first member is not debian-binary"));
    }
    let mut result = Index { read_limits: Some(*limits), ..Index::default() };
    for outer_entry in &outer.entries {
        let Some((control, compression)) = classify(&outer_entry.path) else {
            let path = utf8(&outer_entry.path);
            if path.starts_with("control.tar.") || path.starts_with("data.tar.") {
                return Err(error(format!("Unsupported deb tar compression in '{path}'")));
            }
            let inherits_old_mode = result.get(&outer_entry.path)
                .is_some_and(|old| old.directory == outer_entry.directory && outer_entry.mode.is_none());
            if result.upsert(outer_entry.clone())?.is_some() && !inherits_old_mode {
                result.read_mode_overrides.remove(&outer_entry.path);
            }
            continue;
        };
        if outer_entry.directory
            || !matches!(outer_entry.storage, Some(Storage::Slice { .. }) | Some(Storage::ReadMember(_)))
        {
            return Err(error(format!("Invalid deb archive member '{}'", utf8(&outer_entry.path))));
        }
        let compressed = outer.read_file(&outer_entry.path)?;
        let bytes =
            if compression == "none" { compressed } else { decode(compression, &compressed, limits.max_memory)? };
        limits.memory(bytes.len() as u64)?;
        let mut inner = read_tar(bytes, limits)?;
        for entry in inner.entries {
            let raw_mode = inner.read_mode_overrides.remove(&entry.path);
            let entry = if control { prefix_control(entry) } else { entry };
            let path = entry.path.clone();
            let inherits_old_mode = result.get(&path)
                .is_some_and(|old| old.directory == entry.directory && entry.mode.is_none());
            if result.upsert(entry)?.is_some() {
                if let Some(raw_mode) = raw_mode { result.read_mode_overrides.insert(path, raw_mode); }
                else if !inherits_old_mode { result.read_mode_overrides.remove(&path); }
            }
        }
    }
    result.parents()?;
    Ok(result)
}
