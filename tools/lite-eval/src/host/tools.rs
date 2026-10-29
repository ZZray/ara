//! Workspace path checks of the Capability_Test_Host tools (requirement 2.3).
//!
//! Every path a model passes to a `ws_*` tool goes through [`confine`] before
//! the Core implementation sees it. A rejection is a known tool error
//! (`path_outside_workspace`), never an unknown effect.

use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use serde::Serialize;

use crate::{
    digest::refuse_link,
    workspace::{canonical, link_count},
};

/// What the tool will do with the path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intent {
    /// `ws_read_file`, `ws_list_dir`.
    ReadExisting,
    /// `ws_write_file`: the target may not exist yet.
    WriteMaybeNew,
    /// `ws_edit_file`.
    EditExisting,
    /// Root directory of `ws_search_files`.
    SearchRoot,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum PathRejection {
    Empty,
    /// Lexically or after resolving links, the path is not under the root.
    OutsideWorkspace {
        resolved: PathBuf,
    },
    /// The target is a link (or a file with several hard links) that could
    /// alias a file outside the workspace.
    LinkComponent {
        path: PathBuf,
    },
}

impl PathRejection {
    /// Error code returned to the model.
    pub fn code(&self) -> &'static str {
        match self {
            PathRejection::Empty => "empty_path",
            PathRejection::OutsideWorkspace { .. } | PathRejection::LinkComponent { .. } => "path_outside_workspace",
        }
    }
}

/// Join `raw` to `root` when relative and remove `.` and `..` lexically.
fn normalize(root: &Path, raw: &str) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in root.join(raw).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // Never pops a prefix or root; `C:\..` stays `C:\`.
                if matches!(normalized.components().next_back(), Some(Component::Normal(_))) {
                    normalized.pop();
                }
            }
            other => normalized.push(other),
        }
    }
    normalized
}

/// Resolve `raw` against the canonical workspace `root`. Returns the path to
/// hand to the Core tool: the canonical target when it exists, otherwise the
/// normalized path whose deepest existing ancestor is inside `root`.
pub fn confine(root: &Path, raw: &str, intent: Intent) -> Result<PathBuf, PathRejection> {
    if raw.trim().is_empty() {
        return Err(PathRejection::Empty);
    }
    let normalized = normalize(root, raw);
    let outside = || PathRejection::OutsideWorkspace { resolved: normalized.clone() };
    // The deepest existing ancestor (the target itself when it exists) must be
    // inside the root once junctions and symbolic links resolve. Comparing the
    // canonical form also accepts an absolute path that differs only in case.
    let mut existing = normalized.as_path();
    while fs::symlink_metadata(existing).is_err() {
        existing = existing.parent().ok_or_else(outside)?;
    }
    let resolved = canonical(existing).map_err(|_| outside())?;
    if !resolved.starts_with(root) {
        return Err(PathRejection::OutsideWorkspace { resolved });
    }
    if existing != normalized.as_path() {
        // Only plain names remain after normalization.
        let rest = normalized.strip_prefix(existing).map_err(|_| outside())?;
        return Ok(resolved.join(rest));
    }
    let metadata = fs::symlink_metadata(&normalized)
        .map_err(|_| PathRejection::OutsideWorkspace { resolved: normalized.clone() })?;
    let is_link = refuse_link(&normalized, &metadata).is_err();
    let aliased = metadata.is_file() && link_count(&normalized).map_or(true, |links| links != 1);
    match intent {
        // Writing through or replacing a link, or into a hard-linked file, could change a file outside.
        Intent::WriteMaybeNew | Intent::EditExisting | Intent::ReadExisting if is_link || aliased => {
            Err(PathRejection::LinkComponent { path: normalized })
        }
        _ => Ok(resolved),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexical_escapes_and_foreign_roots_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let root = canonical(dir.path()).unwrap().join("ws");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/a.cpp"), b"int a;\n").unwrap();
        assert_eq!(confine(&root, "src/a.cpp", Intent::ReadExisting).unwrap(), root.join("src").join("a.cpp"));
        assert_eq!(
            confine(&root, "./src/../src/./a.cpp", Intent::EditExisting).unwrap(),
            root.join("src").join("a.cpp")
        );
        assert_eq!(
            confine(&root, "new/dir/b.cpp", Intent::WriteMaybeNew).unwrap(),
            root.join("new").join("dir").join("b.cpp")
        );
        assert_eq!(confine(&root, ".", Intent::SearchRoot).unwrap(), root);
        for raw in ["..", "../ws-other/x", "src/../../x", "/etc/passwd", r"C:\Windows\win.ini", r"\x", "C:x"] {
            assert!(
                matches!(confine(&root, raw, Intent::ReadExisting), Err(PathRejection::OutsideWorkspace { .. })),
                "{raw}"
            );
        }
        assert_eq!(confine(&root, "  ", Intent::ReadExisting), Err(PathRejection::Empty));
        assert_eq!(PathRejection::Empty.code(), "empty_path");
    }
}
