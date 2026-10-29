//! Dependency direction of the Lite crates (requirement 27, module M-P4-DEPS).
//!
//! [`metadata`] reads one workspace's resolved graph from
//! `cargo metadata --locked --offline --format-version 1`. [`check_core`]
//! (27.1) and [`check_harness`] (27.2) walk every normal, build and dev edge
//! reachable from the root package and report each violation with the
//! workspace, the package and the chain of package IDs from the root.

use std::{
    collections::{BTreeMap, VecDeque},
    fmt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{bail, ensure, Context, Result};
use serde::Deserialize;

/// ARA product crates (the root workspace members and their vendored crates)
/// plus the product families named by requirement 27.1. A trailing `*` is a
/// prefix pattern; other entries match the package name exactly, so
/// `ara-agent` does not match `ara-agent-lite-core`.
pub const PRODUCT_CRATES: &[&str] = &[
    "handwave*",
    "lantern*",
    "paseo*",
    "lumen*",
    "ara-agent",
    "ara-workflow",
    "ara-ai",
    "ara-cli",
    "ara-mcp",
    "ara-context",
    "ara-ctok",
    "ara-discovery",
    "ara-prompt",
    "ara-rpc",
    "ara-session",
    "ara-snapcompact",
    "ara-tools",
    "ara-testkit",
    "ara-walk",
    "ara-ast",
    "ara-diff",
    "ara-edit",
    "ara-iso",
    "ara-task-vcs",
];

/// Where a resolved package comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// Any registry (`registry+…` or `sparse+…`).
    Registry,
    /// `rev` is `Some` only when the dependency names a commit hash
    /// (`?rev=<7–40 hex digits>`); a branch, a tag or the default branch is `None`.
    Git { url: String, rev: Option<String> },
    /// Local package; the directory holding its `Cargo.toml`.
    Path(PathBuf),
}

impl fmt::Display for Source {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Registry => formatter.write_str("registry"),
            Source::Git { url, rev: Some(rev) } => write!(formatter, "git {url} rev {rev}"),
            Source::Git { url, rev: None } => write!(formatter, "git {url} (branch, tag or default branch)"),
            Source::Path(path) => write!(formatter, "path {}", path.display()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package {
    pub id: String,
    pub name: String,
    pub source: Source,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DepKind {
    Normal,
    Build,
    Dev,
}

/// Resolved dependency graph of one workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Graph {
    /// Workspace label used in violations (its root directory).
    pub workspace: String,
    /// Package ID of the root package.
    pub root: String,
    pub packages: BTreeMap<String, Package>,
    /// Package ID to its dependencies, one entry per (dependency, kind).
    pub edges: BTreeMap<String, Vec<(String, DepKind)>>,
}

/// A chain is the package IDs from the root to the violating package, inclusive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Violation {
    ForbiddenPackage { workspace: String, package: String, chain: Vec<String> },
    OutsidePathDependency { workspace: String, package: String, chain: Vec<String> },
    GitBranchDependency { workspace: String, package: String, chain: Vec<String> },
    CoreInstances { workspace: String, count: usize },
    CoreNotLocalPath { workspace: String, source: String, chain: Vec<String> },
}

/// `name@version` from a cargo package ID, for readable chains.
pub fn short_id(id: &str) -> String {
    let Some((url, fragment)) = id.rsplit_once('#') else { return id.to_owned() };
    if fragment.contains('@') {
        return fragment.to_owned();
    }
    let url = url.split('?').next().unwrap_or(url);
    let name = url.trim_end_matches('/').rsplit('/').next().unwrap_or(url).trim_end_matches(".git");
    format!("{name}@{fragment}")
}

impl fmt::Display for Violation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let chain = |chain: &[String]| chain.iter().map(|id| short_id(id)).collect::<Vec<_>>().join(" -> ");
        match self {
            Violation::ForbiddenPackage { workspace, package, chain: path } => {
                write!(formatter, "[{workspace}] forbidden package `{package}`: {}", chain(path))
            }
            Violation::OutsidePathDependency { workspace, package, chain: path } => {
                write!(
                    formatter,
                    "[{workspace}] path dependency `{package}` outside the Core workspace: {}",
                    chain(path)
                )
            }
            Violation::GitBranchDependency { workspace, package, chain: path } => {
                write!(formatter, "[{workspace}] git dependency `{package}` not pinned to a commit: {}", chain(path))
            }
            Violation::CoreInstances { workspace, count } => {
                write!(formatter, "[{workspace}] {count} Lite_Core instances in the graph, expected exactly 1")
            }
            Violation::CoreNotLocalPath { workspace, source, chain: path } => {
                write!(
                    formatter,
                    "[{workspace}] Lite_Core comes from {source}, not this repository's path: {}",
                    chain(path)
                )
            }
        }
    }
}

#[derive(Deserialize)]
struct RawMetadata {
    packages: Vec<RawPackage>,
    resolve: Option<RawResolve>,
    workspace_root: String,
}

#[derive(Deserialize)]
struct RawPackage {
    id: String,
    name: String,
    source: Option<String>,
    manifest_path: PathBuf,
}

#[derive(Deserialize)]
struct RawResolve {
    root: Option<String>,
    nodes: Vec<RawNode>,
}

#[derive(Deserialize)]
struct RawNode {
    id: String,
    #[serde(default)]
    deps: Vec<RawDep>,
}

#[derive(Deserialize)]
struct RawDep {
    pkg: String,
    #[serde(default)]
    dep_kinds: Vec<RawDepKind>,
}

#[derive(Deserialize)]
struct RawDepKind {
    kind: Option<String>,
}

fn parse_source(raw: Option<&str>, manifest_path: &Path) -> Result<Source> {
    let Some(raw) = raw else {
        let dir = manifest_path.parent().context("package manifest path has no parent")?;
        return Ok(Source::Path(dir.to_path_buf()));
    };
    if raw.starts_with("registry+") || raw.starts_with("sparse+") {
        return Ok(Source::Registry);
    }
    let Some(rest) = raw.strip_prefix("git+") else { bail!("unknown package source `{raw}`") };
    let before_fragment = rest.split('#').next().unwrap_or(rest);
    let (url, query) = before_fragment.split_once('?').unwrap_or((before_fragment, ""));
    let rev = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("rev="))
        .filter(|rev| (7..=40).contains(&rev.len()) && rev.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .map(str::to_owned);
    Ok(Source::Git { url: url.to_owned(), rev })
}

/// Graph from the JSON printed by `cargo metadata --format-version 1`.
pub fn parse_metadata(json: &[u8]) -> Result<Graph> {
    let raw: RawMetadata = serde_json::from_slice(json).context("cargo metadata output is not the expected JSON")?;
    let resolve = raw.resolve.context("cargo metadata has no resolved graph (was --no-deps used?)")?;
    let root = resolve.root.context("workspace has no root package (virtual workspace)")?;
    let mut packages = BTreeMap::new();
    for package in raw.packages {
        let source = parse_source(package.source.as_deref(), &package.manifest_path)
            .with_context(|| format!("package {}", package.id))?;
        packages.insert(package.id.clone(), Package { id: package.id, name: package.name, source });
    }
    let mut edges = BTreeMap::new();
    for node in resolve.nodes {
        let mut out = Vec::new();
        for dep in node.deps {
            ensure!(packages.contains_key(&dep.pkg), "dependency {} is not a listed package", dep.pkg);
            let mut kinds: Vec<DepKind> = dep
                .dep_kinds
                .iter()
                .map(|kind| match kind.kind.as_deref() {
                    None => Ok(DepKind::Normal),
                    Some("build") => Ok(DepKind::Build),
                    Some("dev") => Ok(DepKind::Dev),
                    Some(other) => bail!("unknown dependency kind `{other}`"),
                })
                .collect::<Result<_>>()?;
            // Cargo before 1.41 printed no dep_kinds; those are normal edges.
            if kinds.is_empty() {
                kinds.push(DepKind::Normal);
            }
            kinds.sort();
            kinds.dedup();
            out.extend(kinds.into_iter().map(|kind| (dep.pkg.clone(), kind)));
        }
        edges.insert(node.id, out);
    }
    ensure!(packages.contains_key(&root), "root package {root} is not listed");
    Ok(Graph { workspace: raw.workspace_root, root, packages, edges })
}

/// Resolved graph of the workspace of `manifest`, using its committed
/// `Cargo.lock` unchanged (`--locked`) and no network (`--offline`).
pub fn metadata(manifest: &Path) -> Result<Graph> {
    let output = Command::new("cargo")
        .args(["metadata", "--locked", "--offline", "--format-version", "1", "--manifest-path"])
        .arg(manifest)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("cannot run cargo metadata for {}", manifest.display()))?;
    ensure!(
        output.status.success(),
        "cargo metadata failed for workspace {} (exit {:?}): {}",
        manifest.display(),
        output.status.code(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    parse_metadata(&output.stdout).with_context(|| format!("workspace {}", manifest.display()))
}

impl Graph {
    /// The same packages and edges seen from another root, e.g. the Core node
    /// inside the Harness graph.
    pub fn rooted_at(&self, id: &str) -> Result<Graph> {
        ensure!(self.packages.contains_key(id), "{id} is not a package of {}", self.workspace);
        Ok(Graph { root: id.to_owned(), ..self.clone() })
    }

    /// Name of the root package.
    pub fn root_name(&self) -> &str {
        &self.packages[&self.root].name
    }

    /// Breadth-first over every edge kind: each reachable package ID with the
    /// shortest chain from the root (root first, the package last).
    pub fn reachable(&self) -> BTreeMap<String, Vec<String>> {
        let mut chains = BTreeMap::from([(self.root.clone(), vec![self.root.clone()])]);
        let mut queue = VecDeque::from([self.root.clone()]);
        while let Some(id) = queue.pop_front() {
            let chain = chains[&id].clone();
            for (dep, _) in self.edges.get(&id).into_iter().flatten() {
                if !chains.contains_key(dep) {
                    let mut next = chain.clone();
                    next.push(dep.clone());
                    chains.insert(dep.clone(), next);
                    queue.push_back(dep.clone());
                }
            }
        }
        chains
    }
}

/// Whether `name` matches one forbidden entry (`prefix*` or an exact name).
pub fn is_forbidden(name: &str, forbidden: &[&str]) -> bool {
    forbidden.iter().any(|pattern| match pattern.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => name == *pattern,
    })
}

/// Canonical form when the path exists, else the path as given.
fn normalized(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// 27.1: from the root, no forbidden package, no path package outside
/// `core_root` and no git package without a commit hash. The root itself is
/// not judged.
pub fn check_core(graph: &Graph, core_root: &Path, forbidden: &[&str]) -> Vec<Violation> {
    let core_root = normalized(core_root);
    let workspace = graph.workspace.clone();
    let mut violations = Vec::new();
    for (id, chain) in graph.reachable() {
        if id == graph.root {
            continue;
        }
        let package = &graph.packages[&id];
        let name = package.name.clone();
        if is_forbidden(&package.name, forbidden) {
            violations.push(Violation::ForbiddenPackage {
                workspace: workspace.clone(),
                package: name.clone(),
                chain: chain.clone(),
            });
        }
        match &package.source {
            Source::Path(path) if !normalized(path).starts_with(&core_root) => {
                violations.push(Violation::OutsidePathDependency { workspace: workspace.clone(), package: name, chain })
            }
            Source::Git { rev: None, .. } => {
                violations.push(Violation::GitBranchDependency { workspace: workspace.clone(), package: name, chain })
            }
            _ => {}
        }
    }
    violations
}

/// 27.2: exactly one reachable package named `core_name`, and it is the path
/// package at `core_root`.
pub fn check_harness(graph: &Graph, core_name: &str, core_root: &Path) -> Vec<Violation> {
    let core_root = normalized(core_root);
    let workspace = graph.workspace.clone();
    let cores: Vec<(String, Vec<String>)> =
        graph.reachable().into_iter().filter(|(id, _)| graph.packages[id].name == core_name).collect();
    let mut violations = Vec::new();
    if cores.len() != 1 {
        violations.push(Violation::CoreInstances { workspace: workspace.clone(), count: cores.len() });
    }
    for (id, chain) in cores {
        let source = &graph.packages[&id].source;
        let local = matches!(source, Source::Path(path) if normalized(path) == core_root);
        if !local {
            violations.push(Violation::CoreNotLocalPath {
                workspace: workspace.clone(),
                source: source.to_string(),
                chain,
            });
        }
    }
    violations
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_distinguish_registry_pinned_git_branch_git_and_path() {
        let manifest = Path::new("E:/repo/crates/x/Cargo.toml");
        assert_eq!(
            parse_source(Some("registry+https://github.com/rust-lang/crates.io-index"), manifest).unwrap(),
            Source::Registry
        );
        assert_eq!(parse_source(Some("sparse+https://index.crates.io/"), manifest).unwrap(), Source::Registry);
        let pinned =
            "git+https://github.com/bigfish1913/pi-rust.git?rev=8ec462672d90e62712d96074638efef89436cebe#8ec4626";
        assert_eq!(
            parse_source(Some(pinned), manifest).unwrap(),
            Source::Git {
                url: "https://github.com/bigfish1913/pi-rust.git".into(),
                rev: Some("8ec462672d90e62712d96074638efef89436cebe".into())
            }
        );
        for unpinned in [
            "git+https://example.com/a.git?branch=main#abcdef0",
            "git+https://example.com/a.git#abcdef0",
            "git+https://example.com/a.git?tag=v1#abcdef0",
            "git+https://example.com/a.git?rev=main#abcdef0",
        ] {
            assert!(
                matches!(parse_source(Some(unpinned), manifest).unwrap(), Source::Git { rev: None, .. }),
                "{unpinned}"
            );
        }
        assert_eq!(parse_source(None, manifest).unwrap(), Source::Path(PathBuf::from("E:/repo/crates/x")));
        assert!(parse_source(Some("weird+x"), manifest).is_err());
    }

    #[test]
    fn short_ids_read_as_name_at_version() {
        assert_eq!(
            short_id("path+file:///E:/repos/ara-github/crates/ara-agent-lite-core#0.1.1"),
            "ara-agent-lite-core@0.1.1"
        );
        assert_eq!(short_id("registry+https://github.com/rust-lang/crates.io-index#serde@1.0.229"), "serde@1.0.229");
        assert_eq!(short_id("plain"), "plain");
    }

    #[test]
    fn forbidden_patterns_are_prefix_or_exact() {
        assert!(is_forbidden("handwave-core", PRODUCT_CRATES));
        assert!(is_forbidden("ara-agent", PRODUCT_CRATES));
        assert!(!is_forbidden("ara-agent-lite-core", PRODUCT_CRATES));
        assert!(!is_forbidden("serde", PRODUCT_CRATES));
    }
}
