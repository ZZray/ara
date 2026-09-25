//! Node `path` semantics the discovery code relies on (POSIX).

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

/// `path.resolve(base, p)`.
pub fn resolve_from(base: &Path, p: &Path) -> PathBuf {
    resolve(&base.join(p))
}

/// `calculateDepth`: `cwd.split(sep).length - target.split(sep).length`.
/// Positive for ancestors, 0 for cwd, negative below it.
pub fn calculate_depth(cwd: &Path, target: &Path) -> i64 {
    let count = |p: &Path| p.to_string_lossy().split('/').count() as i64;
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
        assert_eq!(resolve(Path::new("/a/./b/../c/")), PathBuf::from("/a/c"));
        assert_eq!(resolve(Path::new("/..")), PathBuf::from("/"));
        assert_eq!(calculate_depth(Path::new("/r/a/b"), Path::new("/r")), 2);
        assert_eq!(calculate_depth(Path::new("/r"), Path::new("/r/.gemini")), -1);
        assert!(is_within(Path::new("/h"), Path::new("/h/x")) && !is_within(Path::new("/h"), Path::new("/hx")));
    }
}
