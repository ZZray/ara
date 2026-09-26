//! Node `path` semantics the discovery code relies on.

use std::path::{Component, Path, PathBuf};

/// `path.resolve(p)`: absolute (against the process cwd) and lexically
/// normalized (`.` and `..` collapsed, no trailing separator). Symlinks are
/// not resolved.
pub fn resolve(p: &Path) -> PathBuf {
    let joined = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")).join(p)
    };
    let mut out = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(Component::RootDir.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(name) => out.push(name),
        }
    }
    out
}

/// `path.normalize(p)`: collapse `.`, `..` and repeated separators without
/// making the path absolute (leading `..` of a relative path stays).
pub fn normalize(p: &Path) -> PathBuf {
    let mut out: Vec<Component<'_>> = Vec::new();
    for component in p.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match out.last() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir) | Some(Component::Prefix(_)) => {}
                _ => out.push(component),
            },
            other => out.push(other),
        }
    }
    if out.is_empty() {
        return PathBuf::from(".");
    }
    out.iter().map(|c| c.as_os_str()).collect()
}

/// `path.resolve(base, p)`.
pub fn resolve_from(base: &Path, p: &Path) -> PathBuf {
    resolve(&base.join(p))
}

/// `calculateDepth`: component count difference using the host separator.
/// Positive for ancestors, 0 for cwd, negative below it.
pub fn calculate_depth(cwd: &Path, target: &Path) -> i64 {
    let count = |p: &Path| p.components().count() as i64;
    count(cwd) - count(target)
}

/// `child` is `parent` or lies below it (both resolved).
pub fn is_within(parent: &Path, child: &Path) -> bool {
    resolve(child).starts_with(resolve(parent))
}

pub fn same_path(a: &Path, b: &Path) -> bool {
    resolve(a) == resolve(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_and_depth() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert_eq!(resolve(&root.join("a/./b/../c/")), root.join("a/c"));
        assert_eq!(resolve(&root.join("a/..")), root);
        assert_eq!(calculate_depth(&root.join("r").join("a").join("b"), &root.join("r")), 2);
        assert_eq!(calculate_depth(&root.join("r"), &root.join("r").join(".gemini")), -1);
        assert!(is_within(&root.join("h"), &root.join("h/x")) && !is_within(&root.join("h"), &root.join("hx")));
    }
}
