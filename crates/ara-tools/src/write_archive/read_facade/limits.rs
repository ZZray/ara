//! Fixed OMP596f2da ar/limits.ts configured READ resource ceilings.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;

#[derive(Clone, Copy)]
pub(crate) struct ReadLimits {
    pub(crate) max_entries: usize,
    pub(crate) max_memory: u64,
    pub(crate) max_index: u64,
    pub(crate) max_member: u64,
    pub(crate) max_path: usize,
    pub(crate) max_links: usize,
}
impl Default for ReadLimits {
    fn default() -> Self {
        Self {
            max_entries: MAX_ENTRIES,
            max_memory: MAX_MEMORY,
            max_index: MAX_INDEX,
            max_member: MAX_MEMBER,
            max_path: MAX_PATH,
            max_links: MAX_LINKS,
        }
    }
}
impl ReadLimits {
    pub(crate) fn memory(&self, size: u64) -> Result<()> {
        if size > JS_SAFE {
            return Err(error("Archive is too large to read safely"));
        }
        if size > self.max_memory {
            return Err(error(format!(
                "Archive is too large to read in memory ({} > {} limit)",
                crate::read::format_bytes(size),
                crate::read::format_bytes(self.max_memory)
            )));
        }
        Ok(())
    }
    pub(crate) fn index(&self, size: u64, what: &str) -> Result<()> {
        if size > JS_SAFE {
            return Err(error(format!("Invalid archive: {what} has an invalid size")));
        }
        if size > self.max_index {
            return Err(error(format!(
                "Archive {what} is too large ({} > {} limit)",
                crate::read::format_bytes(size),
                crate::read::format_bytes(self.max_index)
            )));
        }
        Ok(())
    }
    pub(crate) fn member(&self, size: u64, path: &WireString) -> Result<()> {
        if size > JS_SAFE {
            return Err(error(format!("Archive member '{}' has an invalid size", shown(path))));
        }
        if size > self.max_member {
            return Err(error(format!(
                "Archive member '{}' is too large to extract in memory ({} > {} limit)",
                shown(path),
                crate::read::format_bytes(size),
                crate::read::format_bytes(self.max_member)
            )));
        }
        Ok(())
    }
    pub(crate) fn entries(&self, count: usize) -> Result<()> {
        if count > self.max_entries {
            Err(error(format!("Archive has too many entries (> {} limit)", self.max_entries)))
        } else {
            Ok(())
        }
    }
    pub(crate) fn path(&self, bytes: usize, field: &str) -> Result<()> {
        if bytes > self.max_path {
            Err(error(format!("Archive {field} exceeds {} bytes", self.max_path)))
        } else {
            Ok(())
        }
    }
}
