//! Fixed OMP596f2da ar/reader.ts indexed immutable READ facade.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
//! Work in progress: registry/format dispatch and READ wrapper join separately.
use super::*;
mod cpio;
mod arj;
mod cab;
mod deb;
mod formats;
mod limits;
pub(super) mod legacy_limits;
mod lh_static;
mod lzh;
mod lzx;
mod lzw;
mod mszip;
mod iso;
mod rar;
mod rar4_decoder;
mod rar5_decoder;
mod registry;
mod rpm;
mod sevenzip;
mod sevenzip_decode;
mod source;
mod unix_ar;
pub(crate) use formats::{ReadFormat, path_candidates};
pub(crate) use registry::sniff_archive_format;
pub(super) use limits::ReadLimits;
pub(super) use source::{ReadByteSource, validate_range};

pub(super) fn read_with_source_error(
    source: &Source,
    start: u64,
    len: u64,
    wrap_external: impl FnOnce(ToolError) -> ToolError,
) -> Result<Vec<u8>> {
    source::read_with_source_error(source, start, len, wrap_external)
}

pub(super) trait ReadMemberSource: Send + Sync {
    fn read(&self, size: u64, path: &WireString) -> Result<Vec<u8>>;
}

#[derive(Clone)]
pub(crate) struct Node {
    pub(crate) path: WireString,
    pub(crate) directory: bool,
    pub(crate) size: u64,
    pub(crate) mode: Option<f64>,
    pub(crate) mtime: Option<f64>,
}
#[derive(Clone)]
pub(crate) struct DirectoryEntry {
    pub(crate) name: WireString,
    pub(crate) node: Node,
}
pub(crate) struct Member {
    pub(crate) node: Node,
    pub(crate) bytes: Vec<u8>,
}
pub(crate) struct Reader {
    index: Index,
}
impl Reader {
    pub(crate) fn open(path: &Path, ctx: &crate::ToolContext) -> Result<Self> {
        let format = formats::format_from_path(&path.to_string_lossy())
            .ok_or_else(|| error(format!("Unsupported archive format: {}", path.display())))?;
        Self::open_source(source::source_from_path(path)?, format, Some(path), ReadLimits::default(),
            |year, month, day, hour, minute, second| ctx.archive_date_ms(year, month, day, hour, minute, second))
    }
    pub(crate) fn open_source(
        source: Source,
        format: ReadFormat,
        archive_path: Option<&Path>,
        limits: ReadLimits,
        local_date: impl Fn(i32, u32, u32, u32, u32, u32) -> Result<f64>,
    ) -> Result<Self> {
        let index = registry::read_index(source, format, archive_path, limits, local_date)?;
        Self::from_index(index, limits)
    }
    // Format decoders remain private. READ retains the index and the source
    // behind it, never calling WRITE read_members/materialize-all.
    fn from_index(mut index: Index, limits: ReadLimits) -> Result<Self> {
        index.read_limits = Some(limits);
        index.parents()?;
        Ok(Self { index })
    }
    pub(crate) fn get_node(&self, sub_path: &WireString) -> Result<Option<Node>> {
        let Some(normalized) = normalize(sub_path, true) else { return Ok(None) };
        if normalized.is_empty() {
            return Ok(Some(Node { path: normalized, directory: true, size: 0, mode: None, mtime: None }));
        }
        let resolved = self.index.resolve_link(&normalized)?;
        if resolved.is_empty() {
            return Ok(Some(Node { path: normalized, directory: true, size: 0, mode: None, mtime: None }));
        }
        Ok(self.index.get(&resolved).map(|entry| Node {
            path: normalized,
            directory: entry.directory,
            size: entry.size,
            mode: self.index.read_mode(&resolved, entry),
            mtime: entry.mtime,
        }))
    }
    // The READ owner supplies its existing Host ICU comparison on lowercase
    // names. This keeps locale selection at the existing READ owner boundary.
    pub(crate) fn list_directory(
        &self,
        sub_path: &WireString,
        mut compare_names: impl FnMut(&WireString, &WireString) -> std::cmp::Ordering,
    ) -> Result<Vec<DirectoryEntry>> {
        let normalized = normalize(sub_path, true).ok_or_else(|| error("Archive path cannot contain '..'"))?;
        let resolved = if normalized.is_empty() { normalized.clone() } else { self.index.resolve_link(&normalized)? };
        if !normalized.is_empty() && !resolved.is_empty() {
            let entry = self
                .index
                .get(&resolved)
                .ok_or_else(|| error(format!("Archive path '{}' not found", utf8(&normalized))))?;
            if !entry.directory {
                return Err(error(format!("Archive path '{}' is not a directory", utf8(&normalized))));
            }
        }
        let mut prefix = resolved.units().to_vec();
        if !prefix.is_empty() {
            prefix.push(47);
        }
        let mut children = Vec::new();
        let mut seen = HashSet::new();
        for entry in &self.index.entries {
            if !resolved.is_empty() && (!entry.path.units().starts_with(&prefix) || entry.path == resolved) {
                continue;
            }
            let relative = if resolved.is_empty() { entry.path.units() } else { &entry.path.units()[prefix.len()..] };
            let end = relative.iter().position(|&c| c == 47).unwrap_or(relative.len());
            if end == 0 {
                continue;
            }
            let name = WireString::from_units(relative[..end].to_vec());
            let path = join(&normalized, &name);
            if !seen.insert(path.clone()) {
                continue;
            }
            let source_child = join(&resolved, &name);
            let resolved_child = self.index.resolve_link(&source_child)?;
            let child = if resolved_child.is_empty() { None } else { self.index.get(&resolved_child) };
            let directory = resolved_child.is_empty() || child.is_some_and(|c| c.directory) || end < relative.len();
            children.push(DirectoryEntry {
                name,
                node: Node {
                    path,
                    directory,
                    size: if directory { 0 } else { child.map_or(entry.size, |c| c.size) },
                    mode: child
                        .and_then(|c| self.index.read_mode(&resolved_child, c))
                        .or_else(|| self.index.read_mode(&entry.path, entry)),
                    mtime: child.and_then(|c| c.mtime).or(entry.mtime),
                },
            });
        }
        // Stable order is significant when Host localeCompare reports equality.
        children.sort_by(|left, right| compare_names(&left.name, &right.name));
        Ok(children)
    }
    pub(crate) fn read_file(&self, sub_path: &WireString) -> Result<Member> {
        // The shared extraction path owns selected-member integrity/size checks.
        let bytes = self.index.read_file(sub_path)?;
        let node = self.get_node(sub_path)?.ok_or_else(|| error("Archive file path is required"))?;
        Ok(Member { node, bytes })
    }
}

#[cfg(test)]
mod original_archive_tests;
