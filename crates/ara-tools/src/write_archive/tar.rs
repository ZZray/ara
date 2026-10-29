//! Fixed OMP596f2da TAR GNU/ustar/PAX read-for-rewrite and encoder.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;
use super::read_facade::{ReadLimits, legacy_limits::Policy};
const BLOCK: usize = 512;
fn text(bytes: &[u8]) -> WireString {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    decoded(&bytes[..end])
}
fn decoded(bytes: &[u8]) -> WireString {
    let (value, _, _) = encoding_rs::UTF_8.decode(bytes);
    value.into_owned().into()
}
fn numeric(bytes: &[u8]) -> f64 {
    if bytes[0] & 0x80 != 0 {
        let mut n = (bytes[0] & 0x7f) as u128;
        for &b in &bytes[1..] {
            n = n * 256 + b as u128;
        }
        if bytes[0] & 0x40 != 0 { (n as i128 - (1i128 << (bytes.len() * 8 - 1))) as f64 } else { n as f64 }
    } else {
        let mut n = 0u128;
        for &b in bytes {
            if (b'0'..=b'7').contains(&b) {
                n = n * 8 + (b - b'0') as u128;
            }
        }
        n as f64
    }
}
fn size(n: f64, field: &str) -> Result<u64> {
    if !n.is_finite() || n < 0.0 || n.fract() != 0.0 || n > JS_SAFE as f64 {
        Err(error(format!("Invalid tar {field}")))
    } else {
        Ok(n as u64)
    }
}
fn pad(size: u64) -> Result<u64> {
    size.checked_add((512 - size % 512) % 512).filter(|&n| n <= JS_SAFE).ok_or_else(|| error("Invalid tar member size"))
}
fn checksum(b: &[u8]) -> bool {
    let stored = numeric(&b[148..156]);
    let mut unsigned = 0i64;
    let mut signed = 0i64;
    for (i, &byte) in b.iter().enumerate() {
        let byte = if (148..156).contains(&i) { 32 } else { byte };
        unsigned += byte as i64;
        signed += (byte as i8) as i64;
    }
    stored == unsigned as f64 || stored == signed as f64
}
fn metadata_path(bytes: &[u8], field: &str, policy: Policy) -> Result<WireString> {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    policy.path_bytes(end, field)?;
    Ok(text_for_read(&bytes[..end], policy))
}
fn pax(bytes: &[u8], policy: Policy) -> Result<HashMap<String, WireString>> {
    policy.index(bytes.len() as u64, "tar PAX metadata")?;
    let mut map = HashMap::new();
    let mut pos = 0;
    while pos < bytes.len() {
        let space = bytes[pos..]
            .iter()
            .position(|&b| b == 32)
            .map(|i| i + pos)
            .ok_or_else(|| error("Invalid tar PAX record"))?;
        if space == pos || space - pos > 16 {
            return Err(error("Invalid tar PAX record"));
        }
        let mut len = 0usize;
        for &b in &bytes[pos..space] {
            if !b.is_ascii_digit() {
                return Err(error("Invalid tar PAX record"));
            }
            len = len
                .checked_mul(10)
                .and_then(|n| n.checked_add((b - b'0') as usize))
                .filter(|n| *n <= bytes.len() - pos)
                .ok_or_else(|| error("Invalid tar PAX record"))?;
        }
        if len == 0 || pos + len > bytes.len() || bytes[pos + len - 1] != 10 || space + 1 > pos + len - 1 {
            return Err(error("Invalid tar PAX record"));
        }
        let record = &bytes[space + 1..pos + len - 1];
        if let Some(equal) = record.iter().position(|&b| b == 61) {
            let key = &record[..equal];
            let value = &record[equal + 1..];
            if key.starts_with(b"GNU.sparse.") {
                map.insert("GNU.sparse.".into(), if value.is_empty() { "".into() } else { "1".into() });
                if key == b"GNU.sparse.name" {
                    policy.path_bytes(value.len(), "PAX sparse path")?;
                    map.insert("GNU.sparse.name".into(), decoded_for_read(value, policy));
                } else if key == b"GNU.sparse.realsize" || key == b"GNU.sparse.size" {
                    if value.len() > 32 {
                        return Err(error("Invalid tar sparse real size"));
                    }
                    map.insert("GNU.sparse.realsize".into(), decoded_for_read(value, policy));
                }
            } else if key == b"path" || key == b"linkpath" {
                policy.path_bytes(value.len(), if key == b"path" { "PAX path" } else { "PAX link target" })?;
                map.insert(String::from_utf8_lossy(key).into_owned(), decoded_for_read(value, policy));
            } else if key == b"size" {
                if value.len() > 32 {
                    return Err(error("Invalid tar member size"));
                }
                map.insert("size".into(), decoded_for_read(value, policy));
            }
        }
        pos += len;
    }
    Ok(map)
}
fn pax_size(value: &WireString, field: &str) -> Result<u64> {
    let value = utf8(value);
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(error(format!("Invalid tar {field}")));
    }
    size(value.parse::<f64>().map_err(|_| error(format!("Invalid tar {field}")))?, field)
}
#[derive(Clone)]
struct Pending {
    hard: bool,
    target: WireString,
    order: usize,
}
fn prefixes(path: &WireString) -> Vec<WireString> {
    let mut ends = vec![path.len()];
    ends.extend(path.units().iter().enumerate().rev().filter_map(|(i, &u)| (u == 47 && i > 0).then_some(i)));
    ends.into_iter().map(|end| WireString::from_units(path.units()[..end].to_vec())).collect()
}
fn oldgnu(bytes: &[u8], index: &mut Index, pending: &mut HashMap<WireString, Pending>, policy: Policy) -> Result<()> {
    policy.index(bytes.len() as u64, "old-GNU name metadata")?;
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    for line in bytes[..end].split(|&b| b == 10) {
        if !line.starts_with(b"Rename ") {
            continue;
        }
        let split = line
            .windows(4)
            .enumerate()
            .skip(7)
            .find_map(|(i, b)| (b == b" to ").then_some(i))
            .ok_or_else(|| error("Invalid old-GNU name record"))?;
        let from = metadata_path(&line[7..split], "old-GNU source path", policy)?;
        let target_end = if line.last() == Some(&47) { line.len() - 1 } else { line.len() };
        let to = metadata_path(&line[split + 4..target_end], "old-GNU target path", policy)?;
        let mut names = Vec::new();
        for (raw, field) in [(&from, "source path"), (&to, "target path")] {
            let p = portable(raw);
            if starts_slash(&p) {
                return Err(error(format!("Invalid old-GNU {field}")));
            }
            let p = normalize(&p, false).ok_or_else(|| error(format!("Invalid old-GNU {field}")))?;
            policy.path_string(&p, field)?;
            names.push(p);
        }
        let from = &names[0];
        let to = &names[1];
        let prefix = join(from, &"".into());
        let under = |p: &WireString| {
            p == from || (p.units().starts_with(prefix.units()) && p.units().get(from.len()) == Some(&47))
        };
        let mut moved = Vec::new();
        let mut old_modes = if policy.0.is_some() {
            std::mem::take(&mut index.read_mode_overrides)
        } else { HashMap::new() };
        let previous = std::mem::take(&mut index.entries);
        index.by_path.clear();
        for entry in previous {
            let raw_mode = old_modes.remove(&entry.path);
            if under(&entry.path) {
                let link = pending.remove(&entry.path);
                moved.push((entry, link, raw_mode));
            } else {
                let path = entry.path.clone();
                index.upsert(entry)?;
                if let Some(raw_mode) = raw_mode { index.read_mode_overrides.insert(path, raw_mode); }
            }
        }
        for (mut entry, p, raw_mode) in moved {
            let original = entry.path.clone();
            let mut next = to.units().to_vec();
            next.extend_from_slice(&original.units()[from.len()..]);
            entry.path = WireString::from_units(next);
            policy.path_string(&entry.path, "member path")?;
            pending.remove(&entry.path);
            if let Some(p) = p {
                pending.insert(entry.path.clone(), p);
            }
            if let Some(&slot) = index.by_path.get(&entry.path) {
                index.read_mode_overrides.remove(&entry.path);
                if let Some(raw_mode) = raw_mode { index.read_mode_overrides.insert(entry.path.clone(), raw_mode); }
                index.entries[slot] = entry;
            } else {
                let path = entry.path.clone();
                index.upsert(entry)?;
                if let Some(raw_mode) = raw_mode { index.read_mode_overrides.insert(path, raw_mode); }
            }
        }
        for p in pending.values_mut() {
            if p.hard && under(&p.target) {
                let mut next = to.units().to_vec();
                next.extend_from_slice(&p.target.units()[from.len()..]);
                p.target = WireString::from_units(next);
            }
        }
    }
    Ok(())
}
fn resolve_pending(index: &mut Index, pending: &HashMap<WireString, Pending>) -> Result<()> {
    let mut directories = HashSet::new();
    for entry in &index.entries {
        for path in prefixes(&entry.path).into_iter().skip(1) {
            if !directories.insert(path) {
                break;
            }
        }
    }
    let mut unresolved: HashSet<WireString> = pending.keys().cloned().collect();
    let mut dependents: HashMap<WireString, Vec<WireString>> = HashMap::new();
    // Map iteration order cannot change the settled result: blockers explicitly
    // schedule dependent records. The retained index owns archive insertion order.
    let mut ordered = pending.iter().map(|(path, pending)| (pending.order, path.clone())).collect::<Vec<_>>();
    ordered.sort_unstable_by_key(|(order, _)| *order);
    let mut queue = ordered.into_iter().map(|(_, path)| path).collect::<Vec<_>>();
    while let Some(path) = queue.pop() {
        if !unresolved.contains(&path) {
            continue;
        }
        let p = &pending[&path];
        let blocker = |target: &WireString, unresolved: &HashSet<WireString>| {
            prefixes(target).into_iter().find(|name| index.by_path.contains_key(name) && unresolved.contains(name))
        };
        let mut blocked = blocker(&p.target, &unresolved);
        let mut target = p.target.clone();
        if blocked.is_none() {
            if let Ok(resolved) = index.resolve_link(&target) {
                target = resolved;
            }
            if target != p.target {
                blocked = blocker(&target, &unresolved);
            }
        }
        if let Some(blocked) = &blocked {
            if blocked != &path {
                dependents.entry(blocked.clone()).or_default().push(path);
                continue;
            }
        }
        unresolved.remove(&path);
        if let Some(waiting) = dependents.remove(&path) {
            queue.extend(waiting);
        }
        let i = index.by_path[&path];
        if blocked.as_ref() == Some(&path) {
            if p.hard {
                return Err(error(format!(
                    "Archive hard link '{}' has a cyclic target '{}'",
                    shown(&path),
                    shown(&p.target)
                )));
            }
            index.entries[i].storage = Some(Storage::Link { target: p.target.clone(), resolve: false });
            continue;
        }
        if let Some(target_entry) = index.get(&target) {
            if !target_entry.directory && !unresolved.contains(&target) && target_entry.storage.is_some() {
                let target_entry = target_entry.clone();
                index.entries[i].size = target_entry.size;
                index.entries[i].storage = target_entry.storage;
                continue;
            }
        }
        let target_entry = index.get(&target);
        let directory = target.is_empty() || target_entry.is_some_and(|e| e.directory) || directories.contains(&target);
        if !directory {
            if !p.hard {
                index.entries[i].storage = Some(Storage::Link { target: p.target.clone(), resolve: false });
                continue;
            }
            return Err(error(format!(
                "Archive hard link '{}' targets {} '{}'",
                shown(&path),
                if target_entry.is_some() { "unreadable member" } else { "missing member" },
                shown(&p.target)
            )));
        }
        if p.hard {
            return Err(error(format!(
                "Archive hard link '{}' targets directory '{}'",
                shown(&path),
                shown(&p.target)
            )));
        }
        index.entries[i].directory = true;
        index.entries[i].storage = Some(Storage::Link { target: p.target.clone(), resolve: false });
    }
    if !unresolved.is_empty() {
        return Err(error("Archive contains cyclic or unsupported links"));
    }
    Ok(())
}
pub(super) fn read(bytes: Vec<u8>) -> Result<Index> {
    read_impl(bytes, Policy::default())
}
pub(super) fn read_for_read(bytes: Vec<u8>, limits: ReadLimits) -> Result<Index> {
    read_impl(bytes, Policy(Some(limits)))
}
fn read_impl(bytes: Vec<u8>, policy: Policy) -> Result<Index> {
    policy.memory(bytes.len() as u64)?;
    let bytes = Arc::new(bytes);
    let source = Source::Memory(bytes.clone());
    let mut index = Index::default();
    index.read_limits = policy.0;
    let mut pending: HashMap<WireString, Pending> = HashMap::new();
    let mut next_link_order = 0usize;
    let mut offset = 0usize;
    let mut long_name = None;
    let mut long_link = None;
    let mut local: Option<HashMap<String, WireString>> = None;
    let mut global: HashMap<String, WireString> = HashMap::new();
    let mut terminated = false;
    while offset + BLOCK <= bytes.len() {
        let header = &bytes[offset..offset + BLOCK];
        if header.iter().all(|&b| b == 0) {
            terminated = true;
            break;
        }
        if !checksum(header) {
            return Err(error("Invalid or corrupt tar archive header"));
        }
        let kind = if header[156] == 0 { b'0' } else { header[156] };
        let mut size = size(numeric(&header[124..136]), "member size")?;
        let mut name = text_for_read(&header[..100], policy);
        let mut link = text_for_read(&header[157..257], policy);
        if &header[257..263] == b"ustar\0" && &header[263..265] == b"00" {
            let prefix = text_for_read(&header[345..500], policy);
            if !prefix.is_empty() {
                name = join(&prefix, &name);
            }
        }
        let mtime = numeric(&header[136..148]);
        let mode = numeric(&header[100..108]);
        let raw_mode = (policy.0.is_some() && mode.is_finite() && mode >= 0.0
            && mode.fract() == 0.0 && mode <= JS_SAFE as f64 && mode > u32::MAX as f64)
            .then_some(mode as u64);
        let mode =
            (mode.is_finite() && mode >= 0.0 && mode.fract() == 0.0 && mode <= JS_SAFE as f64).then_some(mode as u32);
        offset += BLOCK;
        let blocks = pad(size)?;
        if blocks > bytes.len().saturating_sub(offset) as u64 {
            return Err(error("Archive member data is truncated"));
        }
        let data = &bytes[offset..offset + size as usize];
        match kind {
            b'L' => {
                policy.index(size, "GNU long-name metadata")?;
                long_name = Some(metadata_path(data, "GNU long path", policy)?);
                offset += blocks as usize;
                continue;
            }
            b'K' => {
                policy.index(size, "GNU long-link metadata")?;
                long_link = Some(metadata_path(data, "GNU long link target", policy)?);
                offset += blocks as usize;
                continue;
            }
            b'N' => {
                oldgnu(data, &mut index, &mut pending, policy)?;
                offset += blocks as usize;
                continue;
            }
            b'x' | b'X' => {
                local = Some(pax(data, policy)?);
                offset += blocks as usize;
                continue;
            }
            b'g' => {
                for (key, value) in pax(data, policy)? {
                    if value.is_empty() {
                        global.remove(&key);
                    } else {
                        global.insert(key, value);
                    }
                }
                offset += blocks as usize;
                continue;
            }
            b'M' => return Err(error("GNU multi-volume tar members are not supported")),
            _ => {}
        }
        if let Some(value) = long_name.take() {
            name = value;
        }
        if let Some(value) = long_link.take() {
            link = value;
        }
        let attribute = |key: &str| local.as_ref().and_then(|p| p.get(key)).or_else(|| global.get(key));
        if let Some(value) = attribute("path") {
            name = value.clone();
        }
        if let Some(value) = attribute("linkpath") {
            link = value.clone();
        }
        if let Some(value) = attribute("size") {
            size = pax_size(value, "member size")?;
        }
        if let Some(value) = attribute("GNU.sparse.name") {
            name = value.clone();
        }
        let display_size =
            attribute("GNU.sparse.realsize").map(|v| pax_size(v, "sparse real size")).transpose()?.unwrap_or(size);
        let sparse = kind == b'S' || attribute("GNU.sparse.").is_some_and(|s| s.equals_ascii("1"));
        if kind == b'S' && header[482] == 1 {
            let mut extended = true;
            while extended {
                if offset + BLOCK > bytes.len() {
                    return Err(error("Archive sparse metadata is truncated"));
                }
                extended = bytes[offset + 504] == 1;
                offset += BLOCK;
            }
        }
        let data_offset = offset;
        let blocks = pad(size)?;
        if blocks > bytes.len().saturating_sub(data_offset) as u64 {
            return Err(error(format!("Archive member '{}' is truncated", shown(&name))));
        }
        offset += blocks as usize;
        local = None;
        let directory = kind == b'5' || name.units().last() == Some(&47);
        let Some(path) = normalize(&name, false) else {
            continue;
        };
        policy.path_string(&path, "member path")?;
        let scaled = mtime * 1000.0;
        let mtime = (mtime != 0.0 && scaled.is_finite() && scaled.fract() == 0.0 && scaled.abs() <= JS_SAFE as f64)
            .then_some(scaled);
        let mut entry = Entry {
            path: path.clone(),
            directory,
            size: if directory { 0 } else { display_size },
            mode,
            mtime,
            storage: None,
        };
        let mut waiting = None;
        if !directory && (kind == b'1' || kind == b'2') {
            let hard = kind == b'1';
            let target = portable(&link);
            policy.path_string(&target, "link target")?;
            let resolved = if hard {
                normalize(&target, false)
            } else if starts_slash(&target) {
                None
            } else {
                normalize(&posix_join(&dirname(&path), &target), true)
            };
            entry.size = 0;
            if let Some(target) = resolved.filter(|t| path_len(t) <= policy.max_path()) {
                waiting = Some(Pending { hard, target, order: next_link_order });
                next_link_order += 1;
            } else if hard {
                return Err(error(format!("Archive hard link '{}' has an invalid target", shown(&path))));
            } else {
                entry.storage = Some(Storage::Link { target, resolve: false });
            }
        } else if !directory {
            if !matches!(kind, b'0' | b'7' | b'S') {
                continue;
            }
            policy.member(display_size, &path)?;
            entry.storage = Some(if sparse {
                Storage::Sparse
            } else {
                Storage::Slice {
                    source: source.clone(),
                    offset: data_offset as u64,
                    size: display_size,
                    integrity: None,
                }
            });
        }
        let inherits_old_mode = index.get(&path)
            .is_some_and(|old| old.directory == entry.directory && entry.mode.is_none());
        if index.upsert(entry)?.is_some() {
            if let Some(raw_mode) = raw_mode {
                index.read_mode_overrides.insert(path.clone(), raw_mode);
            } else if !inherits_old_mode {
                index.read_mode_overrides.remove(&path);
            }
            pending.remove(&path);
            if let Some(waiting) = waiting {
                pending.insert(path, waiting);
            }
        }
    }
    if !terminated {
        return Err(error("Not a valid tar archive: missing terminating zero block"));
    }
    resolve_pending(&mut index, &pending)?;
    Ok(index)
}
fn decoded_for_read(bytes: &[u8], policy: Policy) -> WireString {
    if policy.0.is_none() { return decoded(bytes); }
    encoding_rs::UTF_8.decode_with_bom_removal(bytes).0.into_owned().into()
}
fn text_for_read(bytes: &[u8], policy: Policy) -> WireString {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    decoded_for_read(&bytes[..end], policy)
}
pub(super) fn sniff(bytes: &[u8]) -> bool {
    sniff_impl(bytes, Policy::default())
}
pub(super) fn sniff_for_read(bytes: &[u8], limits: ReadLimits) -> bool {
    sniff_impl(bytes, Policy(Some(limits)))
}
fn sniff_impl(bytes: &[u8], policy: Policy) -> bool {
    if bytes.len() < 512 {
        return false;
    }
    if bytes[..512].iter().all(|&b| b == 0) {
        return true;
    }
    !text_for_read(&bytes[..100], policy).is_empty()
        && size(numeric(&bytes[124..136]), "member size").and_then(pad).is_ok()
        && checksum(&bytes[..512])
}
fn octal(header: &mut [u8], offset: usize, len: usize, value: u64) -> Result<()> {
    let n = format!("{value:o}");
    if n.len() > len - 1 {
        return Err(error("Tar numeric value does not fit its header field"));
    }
    header[offset..offset + len - 1].fill(b'0');
    header[offset + len - 1 - n.len()..offset + len - 1].copy_from_slice(n.as_bytes());
    Ok(())
}
fn header(name: &[u8], prefix: &[u8], size: u64, mode: u64, kind: u8) -> Result<Vec<u8>> {
    if name.len() > 100 || prefix.len() > 155 {
        return Err(error("Tar header field is too long"));
    }
    let mut out = vec![0; 512];
    out[..name.len()].copy_from_slice(name);
    out[345..345 + prefix.len()].copy_from_slice(prefix);
    for (offset, len, value) in [(100, 8, mode), (108, 8, 0), (116, 8, 0), (124, 12, size), (136, 12, 0)] {
        octal(&mut out, offset, len, value)?;
    }
    out[148..156].fill(32);
    out[156] = kind;
    out[257..263].copy_from_slice(b"ustar\0");
    out[263..265].copy_from_slice(b"00");
    let checksum = out.iter().map(|&b| b as u64).sum::<u64>();
    let digits = format!("{checksum:06o}");
    out[148..154].copy_from_slice(digits.as_bytes());
    out[154] = 0;
    out[155] = 32;
    Ok(out)
}
fn append_payload(out: &mut Vec<u8>, bytes: &[u8]) -> Result<()> {
    out.extend_from_slice(bytes);
    let padded = pad(bytes.len() as u64)?;
    out.resize(out.len() + padded as usize - bytes.len(), 0);
    Ok(())
}
fn pax_record(key: &str, value: &str) -> Vec<u8> {
    let body = format!("{key}={value}\n");
    let mut len = body.len() + 2;
    loop {
        let next = len.to_string().len() + 1 + body.len();
        if next == len {
            break;
        }
        len = next;
    }
    format!("{len} {body}").into_bytes()
}
fn append(out: &mut Vec<u8>, path: &WireString, bytes: &[u8], directory: bool, sequence: usize) -> Result<()> {
    let mut path = utf8(path);
    if directory {
        path.push('/');
    }
    let path_bytes = path.as_bytes();
    let split = if path_bytes.len() <= 100 {
        Some((path_bytes, &[][..]))
    } else {
        path_bytes.iter().enumerate().rev().find_map(|(i, &b)| {
            (b == 47 && i <= 155 && path_bytes.len() - i - 1 <= 100).then_some((&path_bytes[i + 1..], &path_bytes[..i]))
        })
    };
    let size = if directory { 0 } else { bytes.len() as u64 };
    let mut records = Vec::new();
    if split.is_none() {
        records.extend(pax_record("path", &path));
    }
    if size > 0o77777777777 {
        records.extend(pax_record("size", &size.to_string()));
    }
    if !records.is_empty() {
        out.extend(header(format!("PaxHeaders/{sequence:08}").as_bytes(), &[], records.len() as u64, 0o644, b'x')?);
        append_payload(out, &records)?;
    }
    let fallback = format!("PaxFile/{sequence:08}");
    let (name, prefix) = split.unwrap_or((fallback.as_bytes(), &[]));
    out.extend(header(
        name,
        prefix,
        if size <= 0o77777777777 { size } else { 0 },
        if directory { 0o755 } else { 0o644 },
        if directory { b'5' } else { b'0' },
    )?);
    if !directory {
        append_payload(out, bytes)?;
    }
    Ok(())
}
pub(super) fn encode(members: &[(WireString, Vec<u8>)]) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut kinds: HashMap<WireString, bool> = HashMap::new();
    let mut sequence = 0;
    for (raw, bytes) in members {
        let directory = matches!(raw.units().last(), Some(47 | 92));
        let path = normalize(raw, false).ok_or_else(|| error(format!("Invalid tar member path '{}'", shown(raw))))?;
        if directory && !bytes.is_empty() {
            return Err(error(format!("Tar directory '{}' cannot contain file data", shown(&path))));
        }
        for parent in prefixes(&path).into_iter().skip(1).rev() {
            match kinds.get(&parent) {
                Some(false) => {
                    return Err(error(format!("Tar member '{}' is not a directory", shown(&parent))));
                }
                Some(true) => continue,
                None => {
                    kinds.insert(parent.clone(), true);
                    append(&mut output, &parent, &[], true, sequence)?;
                    sequence += 1;
                }
            }
        }
        if let Some(existing) = kinds.get(&path) {
            if directory && *existing {
                continue;
            }
            return Err(error(format!("Duplicate tar member path '{}'", shown(&path))));
        }
        kinds.insert(path.clone(), directory);
        append(&mut output, &path, bytes, directory, sequence)?;
        sequence += 1;
    }
    output.resize(output.len() + 1024, 0);
    Ok(output)
}
