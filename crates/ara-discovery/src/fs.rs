//! Cached discovery reads (OMP `capability/fs.ts`).
//!
//! Upstream keeps one process-global cache; here a [`FsCache`] belongs to a
//! [`crate::Discovery`] so a host decides its lifetime and invalidation.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// One directory entry as `readdir(withFileTypes)` reports it: the type is
/// the entry's own (a symlink is neither a file nor a directory).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    pub name: OsString,
    pub is_file: bool,
    pub is_dir: bool,
}

#[derive(Default, Debug)]
pub struct FsCache {
    content: Mutex<HashMap<PathBuf, Option<String>>>,
    dirs: Mutex<HashMap<PathBuf, Vec<DirEntry>>>,
}

/// Decode like `Bun.file().text()`: a leading UTF-8 BOM is dropped and
/// invalid sequences become U+FFFD.
fn decode(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    String::from_utf8_lossy(bytes).into_owned()
}

impl FsCache {
    pub fn new() -> Self {
        FsCache::default()
    }

    /// File text, or `None` when missing, unreadable or not a regular file
    /// (a FIFO or socket would block forever). Symlinks are followed.
    pub fn read_file(&self, path: &Path) -> Option<String> {
        let abs = crate::paths::resolve(path);
        if let Some(cached) = self.content.lock().unwrap_or_else(|e| e.into_inner()).get(&abs) {
            return cached.clone();
        }
        let content = match std::fs::metadata(&abs) {
            Ok(meta) if meta.is_file() => std::fs::read(&abs).ok().map(|bytes| decode(&bytes)),
            _ => None,
        };
        self.content.lock().unwrap_or_else(|e| e.into_inner()).insert(abs, content.clone());
        content
    }

    /// Directory entries (empty when missing or not a directory).
    pub fn read_dir_entries(&self, dir: &Path) -> Vec<DirEntry> {
        let abs = crate::paths::resolve(dir);
        if let Some(cached) = self.dirs.lock().unwrap_or_else(|e| e.into_inner()).get(&abs) {
            return cached.clone();
        }
        let entries: Vec<DirEntry> = std::fs::read_dir(&abs)
            .map(|read| {
                read.filter_map(Result::ok)
                    .map(|entry| {
                        let kind = entry.file_type().ok();
                        DirEntry {
                            name: entry.file_name(),
                            is_file: kind.is_some_and(|k| k.is_file()),
                            is_dir: kind.is_some_and(|k| k.is_dir()),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.dirs.lock().unwrap_or_else(|e| e.into_inner()).insert(abs, entries.clone());
        entries
    }

    /// Nearest ancestor of `start` (inclusive) holding a `.git` entry.
    pub fn find_repo_root(&self, start: &Path) -> Option<PathBuf> {
        let mut current = crate::paths::resolve(start);
        loop {
            if self.read_dir_entries(&current).iter().any(|e| e.name == ".git") {
                return Some(current);
            }
            current = current.parent()?.to_path_buf();
        }
    }

    pub fn clear(&self) {
        self.content.lock().unwrap_or_else(|e| e.into_inner()).clear();
        self.dirs.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    /// Forget one path and its parent's listing.
    pub fn invalidate(&self, path: &Path) {
        let abs = crate::paths::resolve(path);
        self.content.lock().unwrap_or_else(|e| e.into_inner()).remove(&abs);
        let mut dirs = self.dirs.lock().unwrap_or_else(|e| e.into_inner());
        dirs.remove(&abs);
        if let Some(parent) = abs.parent() {
            dirs.remove(parent);
        }
    }
}
