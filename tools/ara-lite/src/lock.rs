//! Single-instance data-directory lock (Go: `lock_windows.go` / `lock_unix.go`).
//!
//! The lock is an exclusive OS file lock held for the lifetime of the guard and
//! released by the OS when the process exits, so a crash never leaves a stale lock.

use fs4::fs_std::FileExt;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

#[derive(Debug)]
pub struct DirLock {
    file: File,
}

impl DirLock {
    /// Try to take the exclusive lock on `path`, creating the file if needed.
    pub fn acquire(path: &Path) -> io::Result<DirLock> {
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)?;
        if !FileExt::try_lock_exclusive(&file)? {
            return Err(io::Error::new(io::ErrorKind::WouldBlock, "lock is held by another process"));
        }
        Ok(DirLock { file })
    }
}

impl Drop for DirLock {
    fn drop(&mut self) {
        // Closing the handle releases the lock; the explicit unlock is best effort.
        let _ = FileExt::unlock(&self.file);
    }
}
