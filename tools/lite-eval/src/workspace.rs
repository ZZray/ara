//! Task workspace preparation and isolation checks (requirement 2.2).
//!
//! The workspace is a fresh copy of the fixture: every regular file is
//! copied into a new file, never linked. [`assert_isolated`] reports every
//! channel through which the Oracle could be reached from the workspace or
//! from a model-visible text. This is a Host-side check, not an OS sandbox.

use std::{
    collections::BTreeMap,
    fs::{self, File},
    io,
    path::{Component, Path, PathBuf, Prefix},
};

use anyhow::{ensure, Context, Result};
use serde::Serialize;

use crate::{
    common::sha256_file,
    digest::{refuse_link, tree_digest},
};

/// A prepared workspace. `root` is canonical (see [`canonical`]); `private`
/// is a Host-only sibling directory for build output and receipts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    pub root: PathBuf,
    pub private: PathBuf,
}

/// `fs::canonicalize`, with the Windows verbatim disk prefix (`\\?\D:\`)
/// turned back into `D:\` so the result compares equal to ordinary absolute
/// paths and can be used as a process working directory.
pub fn canonical(path: &Path) -> io::Result<PathBuf> {
    let resolved = fs::canonicalize(path)?;
    let mut components = resolved.components();
    if let Some(Component::Prefix(prefix)) = components.next() {
        if let Prefix::VerbatimDisk(drive) = prefix.kind() {
            let mut plain = PathBuf::from(format!("{}:\\", char::from(drive)));
            plain.extend(components.filter(|component| !matches!(component, Component::RootDir)));
            return Ok(plain);
        }
    }
    Ok(resolved)
}

/// Number of hard links of a regular file.
pub fn link_count(path: &Path) -> Result<u64> {
    let file = File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};
        let mut information: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: the handle belongs to `file`, which outlives the call, and
        // `information` is a properly sized, writable output buffer.
        let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) };
        ensure!(ok != 0, "GetFileInformationByHandle failed for {}: {}", path.display(), io::Error::last_os_error());
        Ok(u64::from(information.nNumberOfLinks))
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(file.metadata()?.nlink())
    }
}

/// Copy every regular file of `fixture` into the new directory
/// `<run_dir>/workspace`; `<run_dir>/host-private` is created alongside.
/// Links in the fixture are refused, as in `lite-eval-tree-v1`.
pub fn prepare(fixture: &Path, run_dir: &Path) -> Result<Workspace> {
    let digest = tree_digest(fixture)?;
    let root = run_dir.join("workspace");
    let private = run_dir.join("host-private");
    fs::create_dir(&root).with_context(|| format!("workspace {} must not exist yet", root.display()))?;
    fs::create_dir(&private).with_context(|| format!("cannot create {}", private.display()))?;
    for entry in &digest.entries {
        let source = if fixture.is_dir() { fixture.join(&entry.path) } else { fixture.to_owned() };
        let target = root.join(&entry.path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&source, &target).with_context(|| format!("cannot copy {}", source.display()))?;
        ensure!(sha256_file(&target)? == entry.sha256, "copy of {} differs from the fixture", entry.path);
    }
    Ok(Workspace { root: canonical(&root)?, private: canonical(&private)? })
}

/// Relative path (with `/`) to SHA-256 of every regular file in the
/// workspace. A link anywhere in the workspace is an error.
pub fn snapshot(workspace: &Workspace) -> Result<BTreeMap<String, String>> {
    Ok(tree_digest(&workspace.root)?.entries.into_iter().map(|entry| (entry.path, entry.sha256)).collect())
}

/// What the model or its tool processes can see besides the workspace.
#[derive(Clone, Debug, Default)]
pub struct Exposure {
    /// (label, text): task description, system prompt, each tool description.
    pub texts: Vec<(String, String)>,
    /// Environment of tool processes.
    pub environment: Vec<(String, String)>,
}

/// One isolation problem. Feature strings are referred to by their 1-based
/// position, so the report itself never contains one.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(tag = "problem", rename_all = "snake_case")]
pub enum IsolationProblem {
    /// The workspace and an Oracle directory are nested.
    Nested {
        workspace: PathBuf,
        oracle_dir: PathBuf,
    },
    /// A symbolic link, junction or other reparse point in the workspace.
    Link {
        path: String,
    },
    /// A workspace file with more than one hard link.
    HardLink {
        path: String,
        links: u64,
    },
    /// A workspace file with the same bytes as an Oracle file.
    OracleCopy {
        path: String,
    },
    OraclePathExposed {
        channel: String,
    },
    FeatureExposed {
        channel: String,
        feature: usize,
    },
}

fn path_forms(path: &Path) -> Vec<String> {
    let text = path.to_string_lossy().to_lowercase();
    vec![text.replace('/', "\\"), text.replace('\\', "/")]
}

/// Requirement 2.2 checks; an empty list means isolated. `oracle_paths` are
/// the Oracle file and the file that registers the feature strings.
pub fn assert_isolated(
    workspace: &Workspace,
    oracle_paths: &[PathBuf],
    features: &[String],
    exposure: &Exposure,
) -> Result<Vec<IsolationProblem>> {
    let mut problems = Vec::new();
    let mut oracle_hashes = Vec::new();
    let mut forms = Vec::new();
    for oracle in oracle_paths {
        let oracle = canonical(oracle).with_context(|| format!("cannot resolve Oracle path {}", oracle.display()))?;
        let directory = oracle.parent().context("Oracle path has no parent directory")?.to_owned();
        if workspace.root.starts_with(&directory) || directory.starts_with(&workspace.root) {
            problems
                .push(IsolationProblem::Nested { workspace: workspace.root.clone(), oracle_dir: directory.clone() });
        }
        oracle_hashes.push(sha256_file(&oracle)?);
        forms.extend(path_forms(&oracle));
        forms.extend(path_forms(&directory));
    }
    let mut pending = vec![(workspace.root.clone(), String::new())];
    while let Some((directory, prefix)) = pending.pop() {
        for item in fs::read_dir(&directory).with_context(|| format!("cannot list {}", directory.display()))? {
            let path = item?.path();
            let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            let relative = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
            let metadata = fs::symlink_metadata(&path)?;
            if refuse_link(&path, &metadata).is_err() {
                problems.push(IsolationProblem::Link { path: relative });
            } else if metadata.is_dir() {
                pending.push((path, relative));
            } else {
                let links = link_count(&path)?;
                if links != 1 {
                    problems.push(IsolationProblem::HardLink { path: relative.clone(), links });
                }
                if oracle_hashes.contains(&sha256_file(&path)?) {
                    problems.push(IsolationProblem::OracleCopy { path: relative });
                }
            }
        }
    }
    let channels = exposure
        .texts
        .iter()
        .map(|(label, text)| (format!("text:{label}"), text))
        .chain(exposure.environment.iter().map(|(name, value)| (format!("env:{name}"), value)));
    for (channel, text) in channels {
        let lowered = text.to_lowercase();
        if forms.iter().any(|form| lowered.contains(form.as_str())) {
            problems.push(IsolationProblem::OraclePathExposed { channel: channel.clone() });
        }
        for (index, feature) in features.iter().enumerate() {
            if !feature.is_empty() && text.contains(feature.as_str()) {
                problems.push(IsolationProblem::FeatureExposed { channel: channel.clone(), feature: index + 1 });
            }
        }
    }
    problems.sort();
    Ok(problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_copies_files_as_new_single_link_files() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = dir.path().join("fixture");
        fs::create_dir_all(fixture.join("src")).unwrap();
        fs::write(fixture.join("src/a.cpp"), b"int a;\n").unwrap();
        fs::write(fixture.join("README.md"), "说明\n").unwrap();
        let run = dir.path().join("run");
        fs::create_dir(&run).unwrap();
        let workspace = prepare(&fixture, &run).unwrap();
        assert!(!workspace.root.to_string_lossy().starts_with(r"\\?\"));
        let files = snapshot(&workspace).unwrap();
        let fixture_files: BTreeMap<_, _> =
            tree_digest(&fixture).unwrap().entries.into_iter().map(|entry| (entry.path, entry.sha256)).collect();
        assert_eq!(files, fixture_files);
        for relative in files.keys() {
            assert_eq!(link_count(&workspace.root.join(relative)).unwrap(), 1);
        }
        assert!(prepare(&fixture, &run).is_err(), "an existing workspace is never reused");
    }
}
