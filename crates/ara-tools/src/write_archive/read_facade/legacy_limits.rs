//! Optional READ ceilings for shared ZIP/TAR/ASAR parsers. WRITE always uses
//! None and delegates to its existing checks; configured READ never clamps to
//! those defaults. This policy is passed explicitly, not process/global state.
use super::*;
#[derive(Clone, Copy, Default)]
pub(in crate::write_archive) struct Policy(pub(in crate::write_archive) Option<ReadLimits>);
impl Policy {
    pub(in crate::write_archive) fn memory(self, size: u64) -> Result<()> {
        match self.0 {
            Some(limits) => limits.memory(size),
            None => super::super::memory_size(size),
        }
    }
    pub(in crate::write_archive) fn index(self, size: u64, what: &str) -> Result<()> {
        match self.0 {
            Some(limits) => limits.index(size, what),
            None => super::super::index_size(size, what),
        }
    }
    pub(in crate::write_archive) fn member(self, size: u64, path: &WireString) -> Result<()> {
        match self.0 {
            Some(limits) => limits.member(size, path),
            None => super::super::member_size(size, path),
        }
    }
    pub(in crate::write_archive) fn count(self, count_value: usize) -> Result<()> {
        match self.0 {
            Some(limits) => limits.entries(count_value),
            None => super::super::count(count_value),
        }
    }
    pub(in crate::write_archive) fn path_bytes(self, size: usize, field: &str) -> Result<()> {
        match self.0 {
            Some(limits) => limits.path(size, field),
            None => super::super::path_bytes(size, field),
        }
    }
    pub(in crate::write_archive) fn path_string(self, path: &WireString, field: &str) -> Result<()> {
        match self.0 {
            Some(limits) => limits.path(path_len(path), field),
            None => super::super::path_string(path, field),
        }
    }
    pub(in crate::write_archive) fn max_path(self) -> usize {
        self.0.map_or(MAX_PATH, |limits| limits.max_path)
    }
}
