//! Fixed OMP 596f2da file-write-fallback.ts and path-utils.ts:731-789 (MIT).
//! Ordinary EDIT byte-write/unlink/mkdir seam only. Handlers and their live
//! extension context are Host-owned; this registry grants no permission.

use std::fmt;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use async_trait::async_trait;

#[derive(Clone, Copy)]
pub struct EditMutationFileType {
    pub is_symlink: bool,
    pub is_directory: bool,
    pub is_file: bool,
}

/// Real syscalls or a controlled fault source. No default successful methods.
pub trait EditMutationFileSystem: Send + Sync {
    fn write(&self, dst: &Path, content: &str) -> io::Result<()>;
    fn unlink(&self, dst: &Path) -> io::Result<()>;
    fn mkdir(&self, directory: &Path) -> io::Result<()>;
    fn realpath(&self, path: &Path) -> io::Result<PathBuf>;
    fn lstat(&self, path: &Path) -> io::Result<EditMutationFileType>;
}

pub struct OsEditMutationFileSystem;
impl EditMutationFileSystem for OsEditMutationFileSystem {
    fn write(&self, dst: &Path, content: &str) -> io::Result<()> {
        std::fs::write(dst, content.as_bytes())
    }
    fn unlink(&self, dst: &Path) -> io::Result<()> {
        std::fs::remove_file(dst)
    }
    fn mkdir(&self, directory: &Path) -> io::Result<()> {
        std::fs::create_dir_all(directory)
    }
    fn realpath(&self, path: &Path) -> io::Result<PathBuf> {
        std::fs::canonicalize(path)
    }
    fn lstat(&self, path: &Path) -> io::Result<EditMutationFileType> {
        let kind = std::fs::symlink_metadata(path)?.file_type();
        Ok(EditMutationFileType { is_symlink: kind.is_symlink(), is_directory: kind.is_dir(), is_file: kind.is_file() })
    }
}

pub struct EditFileWriteFallbackRequest {
    /// Canonical syscall destination, following the final link for writes.
    pub dst: PathBuf,
    pub content: String,
    pub session_id: Option<String>,
    pub cause: Arc<io::Error>,
}

pub struct EditFileDeleteFallbackRequest {
    /// Canonical ancestors, final component preserved for a plain unlink.
    pub dst: PathBuf,
    pub confirmed_file: bool,
    pub session_id: Option<String>,
    pub cause: Arc<io::Error>,
}

#[async_trait]
pub trait EditFileWriteFallback: Send + Sync {
    /// True means these exact bytes are durably written at the resolved dst.
    async fn write(&self, request: &EditFileWriteFallbackRequest) -> io::Result<bool>;
}

#[async_trait]
pub trait EditFileDeleteFallback: Send + Sync {
    /// True means dst is gone. Plain unlink only: never recursive or realpath.
    async fn delete(&self, request: &EditFileDeleteFallbackRequest) -> io::Result<bool>;
}

pub struct EditFallbackHandlerWarning {
    pub operation: &'static str,
    pub dst: PathBuf,
    pub error: String,
}

pub type EditFallbackWarningObserver = Arc<dyn Fn(EditFallbackHandlerWarning) + Send + Sync>;

/// Keeps the exact original typed errno and an optional recovered mkdir cause.
/// Rust io::Error cannot attach a cause while retaining raw_os_error, so callers
/// receive both explicitly instead of replacing the original with a broker error.
#[derive(Debug)]
pub struct EditFileMutationFailure {
    original: Arc<io::Error>,
    boundary_cause: Option<Arc<io::Error>>,
}

impl EditFileMutationFailure {
    pub fn original(&self) -> &io::Error {
        &self.original
    }
    pub fn boundary_cause(&self) -> Option<&io::Error> {
        self.boundary_cause.as_deref()
    }
}
impl fmt::Display for EditFileMutationFailure {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.original.fmt(output)
    }
}
impl std::error::Error for EditFileMutationFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.boundary_cause.as_deref().unwrap_or(&self.original))
    }
}

/// A process-shared registry supplied by the Host to every Session, including
/// restricted subagents. Requests name their issuing Session; lists themselves
/// are not filtered by that identity. Write and delete are separate opt-ins.
pub struct EditFileMutationFallbackRegistry {
    io: Arc<dyn EditMutationFileSystem>,
    warnings: EditFallbackWarningObserver,
    next_id: AtomicU64,
    writes: Mutex<Vec<(u64, Arc<dyn EditFileWriteFallback>)>>,
    deletes: Mutex<Vec<(u64, Arc<dyn EditFileDeleteFallback>)>>,
}

impl EditFileMutationFallbackRegistry {
    pub fn new(io: Arc<dyn EditMutationFileSystem>, warnings: EditFallbackWarningObserver) -> Arc<Self> {
        Arc::new(Self {
            io,
            warnings,
            next_id: AtomicU64::new(1),
            writes: Mutex::new(Vec::new()),
            deletes: Mutex::new(Vec::new()),
        })
    }

    pub fn has_write_fallback(&self) -> bool {
        !self.writes.lock().unwrap_or_else(|e| e.into_inner()).is_empty()
    }
    pub fn has_delete_fallback(&self) -> bool {
        !self.deletes.lock().unwrap_or_else(|e| e.into_inner()).is_empty()
    }

    fn registration_id(&self) -> u64 {
        self.next_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .expect("file fallback registration counter exhausted")
    }

    pub fn add_write(self: &Arc<Self>, handler: Arc<dyn EditFileWriteFallback>) -> EditFileFallbackRegistration {
        let id = self.registration_id();
        self.writes.lock().unwrap_or_else(|e| e.into_inner()).push((id, handler));
        EditFileFallbackRegistration { registry: Arc::downgrade(self), id, write: true }
    }
    pub fn add_delete(self: &Arc<Self>, handler: Arc<dyn EditFileDeleteFallback>) -> EditFileFallbackRegistration {
        let id = self.registration_id();
        self.deletes.lock().unwrap_or_else(|e| e.into_inner()).push((id, handler));
        EditFileFallbackRegistration { registry: Arc::downgrade(self), id, write: false }
    }

    pub fn mkdir_allowing_fallback(&self, directory: &Path) -> io::Result<()> {
        match self.io.mkdir(directory) {
            Err(error) if self.has_write_fallback() && is_permission_denied(&error) => Ok(()),
            result => result,
        }
    }

    fn proven_not_symlink(&self, path: &Path) -> bool {
        match self.io.lstat(path) {
            Ok(kind) => !kind.is_symlink,
            Err(error) => error.kind() == io::ErrorKind::NotFound,
        }
    }

    fn resolve_syscall_target(&self, dst: &Path, follow_final: bool) -> Option<PathBuf> {
        let absolute = std::path::absolute(dst).ok()?;
        let mut target = PathBuf::new();
        for component in absolute.components() {
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    target.pop();
                }
                component => target.push(component.as_os_str()),
            }
        }
        if follow_final {
            if let Ok(real) = self.io.realpath(&target) {
                return Some(real);
            }
            if !self.proven_not_symlink(&target) {
                return None;
            }
        }
        let mut tail = vec![target.file_name()?.to_owned()];
        let mut ancestor = target.parent()?.to_owned();
        loop {
            if let Ok(real) = self.io.realpath(&ancestor) {
                let mut resolved = real;
                for component in tail.iter().rev() {
                    resolved.push(component);
                }
                return Some(resolved);
            }
            if !self.proven_not_symlink(&ancestor) {
                return None;
            }
            let parent = ancestor.parent()?.to_owned();
            if parent == ancestor {
                return None;
            }
            tail.push(ancestor.file_name()?.to_owned());
            ancestor = parent;
        }
    }

    pub async fn write(
        &self,
        dst: &Path,
        content: &str,
        session_id: Option<&str>,
    ) -> Result<(), EditFileMutationFailure> {
        self.write_using(dst, content, session_id, || self.io.write(dst, content)).await
    }

    /// The established update-writer hook may supply the same primitive here;
    /// permission classification never replaces it with a different retry.
    pub async fn write_using<F>(
        &self,
        dst: &Path,
        content: &str,
        session_id: Option<&str>,
        mut primitive: F,
    ) -> Result<(), EditFileMutationFailure>
    where
        F: FnMut() -> io::Result<()> + Send,
    {
        for attempt in 0..2 {
            let original = match primitive() {
                Ok(()) => return Ok(()),
                Err(error) => Arc::new(error),
            };
            if !self.has_write_fallback() {
                return Err(EditFileMutationFailure { original, boundary_cause: None });
            }
            let cause = if is_permission_denied(&original) {
                Some(original.clone())
            } else if attempt == 0 && original.kind() == io::ErrorKind::NotFound {
                match dst.parent().map(|parent| self.io.mkdir(parent)) {
                    Some(Ok(())) => continue,
                    Some(Err(error)) if is_permission_denied(&error) => Some(Arc::new(error)),
                    _ => None,
                }
            } else {
                None
            };
            if let Some(cause) = &cause {
                if let Some(target) = self.resolve_syscall_target(dst, true) {
                    let request = EditFileWriteFallbackRequest {
                        dst: target,
                        content: content.to_owned(),
                        cause: cause.clone(),
                        session_id: session_id.map(str::to_owned),
                    };
                    let handlers = self
                        .writes
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .iter()
                        .map(|(_, handler)| handler.clone())
                        .collect::<Vec<_>>();
                    for handler in handlers {
                        match handler.write(&request).await {
                            Ok(true) => return Ok(()),
                            Ok(false) => {}
                            Err(error) => (self.warnings)(EditFallbackHandlerWarning {
                                operation: "write",
                                dst: request.dst.clone(),
                                error: error.to_string(),
                            }),
                        }
                    }
                }
            }
            let boundary_cause = cause.filter(|cause| !Arc::ptr_eq(cause, &original));
            return Err(EditFileMutationFailure { original, boundary_cause });
        }
        unreachable!("the second failed primitive cannot request another retry")
    }

    pub async fn delete(&self, dst: &Path, session_id: Option<&str>) -> Result<(), EditFileMutationFailure> {
        self.delete_using(dst, session_id, || self.io.unlink(dst)).await
    }

    /// Fixed optional file-handle unlink: preserve the issuing primitive and
    /// run it exactly once through the same denial and target-resolution owner.
    pub async fn delete_using<F>(
        &self,
        dst: &Path,
        session_id: Option<&str>,
        primitive: F,
    ) -> Result<(), EditFileMutationFailure>
    where
        F: FnOnce() -> io::Result<()> + Send,
    {
        let original = match primitive() {
            Ok(()) => return Ok(()),
            Err(error) => Arc::new(error),
        };
        let failure = || EditFileMutationFailure { original: original.clone(), boundary_cause: None };
        if !self.has_delete_fallback() || !is_permission_denied(&original) {
            return Err(failure());
        }
        let target = self.resolve_syscall_target(dst, false).ok_or_else(failure)?;
        let stat = match self.io.lstat(&target) {
            Ok(kind) => Some(kind),
            Err(error) if is_permission_denied(&error) => None,
            Err(_) => return Err(failure()),
        };
        if stat.is_some_and(|kind| kind.is_directory) {
            return Err(failure());
        }
        let request = EditFileDeleteFallbackRequest {
            dst: target,
            cause: original.clone(),
            confirmed_file: stat.is_some_and(|kind| kind.is_file),
            session_id: session_id.map(str::to_owned),
        };
        let handlers = self
            .deletes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|(_, handler)| handler.clone())
            .collect::<Vec<_>>();
        for handler in handlers {
            match handler.delete(&request).await {
                Ok(true) => return Ok(()),
                Ok(false) => {}
                Err(error) => (self.warnings)(EditFallbackHandlerWarning {
                    operation: "delete",
                    dst: request.dst.clone(),
                    error: error.to_string(),
                }),
            }
        }
        Err(failure())
    }
}

/// Host retains this registration until its owning extension/session closes.
/// Disposal removes exactly one registration; iteration uses a prior snapshot.
pub struct EditFileFallbackRegistration {
    registry: Weak<EditFileMutationFallbackRegistry>,
    id: u64,
    write: bool,
}
impl EditFileFallbackRegistration {
    pub fn dispose(&self) {
        if let Some(registry) = self.registry.upgrade() {
            if self.write {
                registry.writes.lock().unwrap_or_else(|e| e.into_inner()).retain(|(id, _)| *id != self.id);
            } else {
                registry.deletes.lock().unwrap_or_else(|e| e.into_inner()).retain(|(id, _)| *id != self.id);
            }
        }
    }
}
impl Drop for EditFileFallbackRegistration {
    fn drop(&mut self) {
        self.dispose();
    }
}

/// Structured errno/kind is authoritative; a message is consulted only for an
/// unstructured Other error, so a pathname containing EACCES is not a denial.
pub fn is_permission_denied(error: &io::Error) -> bool {
    if let Some(code) = error.raw_os_error() {
        #[cfg(unix)]
        return matches!(code, libc::EPERM | libc::EACCES | libc::EROFS);
        #[cfg(windows)]
        return matches!(code, 5 | 19); // Win32 ACCESS_DENIED / WRITE_PROTECT
        #[cfg(not(any(unix, windows)))]
        {
            let _ = code;
            return false;
        }
    }
    match error.kind() {
        io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem => true,
        io::ErrorKind::Other => {
            static MESSAGE: std::sync::LazyLock<regex::Regex> =
                std::sync::LazyLock::new(|| regex::Regex::new(r"(?-u:\b(?:EPERM|EACCES|EROFS)\b)").unwrap());
            MESSAGE.is_match(&error.to_string())
        }
        _ => false,
    }
}
