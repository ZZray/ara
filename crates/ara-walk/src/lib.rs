//! Directory traversal with gitignore semantics (OMP `crates/pi-walker/src/lib.rs`
//! at 596f2da7101178214aa27a753529d15e6b7ad91d: `IgnoreState`, `FastIgnore`,
//! `load_gitignore`, `ignore_line_covers_root`, the per-entry filters of the
//! directory walk, and `WalkOrder::Path`).
//!
//! Each directory's `.ignore` and `.gitignore` apply to paths below it, and
//! `.git/info/exclude` applies inside its repository. Ignore files of the
//! walk root's ancestors apply only up to the nearest repository root (`.git`
//! or `.jj`), minus rules that would hide the walk root itself. The global
//! gitignore applies only inside a repository. Precedence: nearest `.ignore`,
//! then nearest `.gitignore`, then the innermost repository's exclude, then
//! the global file. `.git` is always pruned; symlinks are never followed.
//! Entries are visited depth-first with each directory sorted by name.
//! FIFOs, sockets and devices are skipped. The walk root is canonicalized
//! (upstream `resolve_search_path`), so the ignore chain follows the real path.
//!
//! The explicit owned-entry collector adds metadata, symlink policies and a
//! shared scan cache; the existing `walk` entry keeps its uncached policy.
//! Differences: sequential only (no parallel walker or
//! platform-specific directory reads). Unreadable directories are skipped.
//! Upstream grep does not descend a symlinked directory root (its walker
//! lstats the root and finds a symlink), so it reports no matches there; ARA
//! walks the resolved directory for grep as upstream glob does.

// Ported from Oh My Pi 596f2da7101178214aa27a753529d15e6b7ad91d, MIT.
// Copyright (c) 2025 Mario Zechner
// Copyright (c) 2025-2026 Can Bölük
// Copyright (c) 2026 Stencil Labs, Inc.
// The upstream MIT permission and disclaimer are reproduced in cache.rs.

use ignore::Match;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::ffi::{OsStr, OsString};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, UNIX_EPOCH};
use tokio_util::sync::CancellationToken;

pub mod cache;
pub use cache::{
    cache_ttl_ms, collect_entries, empty_recheck_ms, invalidate_all, invalidate_path, invalidate_path_string,
    max_cache_entries, walk_workers,
};

/// Why a walk stopped before finishing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkError {
    Timeout,
    Aborted,
}

/// Cancellation plus a wall-clock budget, checked between entries.
#[derive(Clone)]
pub struct Budget {
    pub cancel: CancellationToken,
    pub deadline: Instant,
}

impl Budget {
    pub fn check(&self) -> Result<(), WalkError> {
        if self.cancel.is_cancelled() {
            Err(WalkError::Aborted)
        } else if Instant::now() >= self.deadline {
            Err(WalkError::Timeout)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
}

#[derive(Clone, Debug)]
pub struct WalkEntry {
    /// `/`-separated path relative to the walk root.
    pub relative: String,
    /// Under the canonical walk root.
    pub path: PathBuf,
    pub kind: EntryKind,
}

/// Modification time in milliseconds since the epoch; 0 when unknown.
pub fn mtime_ms(meta: &std::fs::Metadata) -> f64 {
    meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0.0, |d| d.as_secs_f64() * 1000.0)
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct WalkOptions {
    pub include_hidden: bool,
    pub use_gitignore: bool,
    pub skip_node_modules: bool,
    /// Deepest entry depth visited (root children are depth 1).
    pub max_depth: usize,
}

/// Metadata collected for each owned entry (fixed pi-walker WalkDetail).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WalkDetail {
    Minimal,
    Full,
}

/// Only the explicit owned-entry collector follows these policies.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FollowLinks {
    Never,
    Roots,
    Always,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CollectOptions {
    pub walk: WalkOptions,
    pub detail: WalkDetail,
    pub follow_links: FollowLinks,
    pub cache: bool,
}

impl CollectOptions {
    /// Collection remains uncached unless a consumer explicitly opts in.
    pub const fn new(walk: WalkOptions) -> Self {
        Self { walk, detail: WalkDetail::Minimal, follow_links: FollowLinks::Never, cache: false }
    }
}

#[derive(Clone, Debug)]
pub struct CollectedEntry {
    pub relative: String,
    pub path: PathBuf,
    pub kind: EntryKind,
    pub mtime_ms: Option<f64>,
    pub size: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct CollectedEntries {
    pub entries: Vec<CollectedEntry>,
    pub cache_age_ms: u64,
}

pub enum Visit {
    Continue,
    Stop,
}

struct IgnoreState {
    parent: Option<Arc<IgnoreState>>,
    ignore: Option<Gitignore>,
    gitignore: Option<Gitignore>,
    git_exclude: Option<Gitignore>,
    has_git: bool,
    chain_has_matchers: bool,
    any_git: bool,
}

impl IgnoreState {
    fn new(
        parent: Option<Arc<IgnoreState>>,
        ignore: Option<Gitignore>,
        gitignore: Option<Gitignore>,
        git_exclude: Option<Gitignore>,
        has_git: bool,
    ) -> Arc<IgnoreState> {
        let parent_matchers = parent.as_ref().is_some_and(|p| p.chain_has_matchers);
        let parent_git = parent.as_ref().is_some_and(|p| p.any_git);
        let has_matchers = ignore.is_some() || gitignore.is_some() || git_exclude.is_some();
        Arc::new(IgnoreState {
            parent,
            ignore,
            gitignore,
            git_exclude,
            has_git,
            chain_has_matchers: has_matchers || parent_matchers,
            any_git: has_git || parent_git,
        })
    }

    /// Walk root: its own files unfiltered, on top of the ancestor chain.
    fn root(root: &Path, use_gitignore: bool) -> Arc<IgnoreState> {
        if !use_gitignore {
            return Self::new(None, None, None, None, false);
        }
        let parent = Self::parents(root);
        let has_git = has_repo_marker(root);
        let exclude = if has_git { load(root, &root.join(".git/info/exclude"), None) } else { None };
        Self::new(
            parent,
            load(root, &root.join(".ignore"), None),
            load(root, &root.join(".gitignore"), None),
            exclude,
            has_git,
        )
    }

    /// Ancestors up to the nearest repository root, outermost first.
    fn parents(root: &Path) -> Option<Arc<IgnoreState>> {
        let ancestors: Vec<&Path> = root.ancestors().skip(1).collect();
        let repo = ancestors.iter().position(|p| has_repo_marker(p))?;
        let mut parent = None;
        for dir in ancestors[..=repo].iter().rev() {
            let has_git = has_repo_marker(dir);
            let exclude = if has_git { load(dir, &dir.join(".git/info/exclude"), Some(root)) } else { None };
            parent = Some(Self::new(
                parent,
                load(dir, &dir.join(".ignore"), Some(root)),
                load(dir, &dir.join(".gitignore"), Some(root)),
                exclude,
                has_git,
            ));
        }
        parent
    }

    /// A subdirectory, from the names seen in its listing.
    fn child(dir: &Path, parent: &Arc<IgnoreState>, names: EntryNames) -> Arc<IgnoreState> {
        if !(names.ignore_file || names.gitignore_file || names.git_dir || names.repo_marker) {
            return Arc::clone(parent);
        }
        Self::new(
            Some(Arc::clone(parent)),
            if names.ignore_file { load(dir, &dir.join(".ignore"), None) } else { None },
            if names.gitignore_file { load(dir, &dir.join(".gitignore"), None) } else { None },
            if names.git_dir { load(dir, &dir.join(".git/info/exclude"), None) } else { None },
            names.repo_marker,
        )
    }
}

#[derive(Clone, Copy, Default)]
struct EntryNames {
    ignore_file: bool,
    gitignore_file: bool,
    git_dir: bool,
    repo_marker: bool,
}

impl EntryNames {
    fn record(&mut self, name: &OsStr, kind: Option<EntryKind>) {
        if matches!(kind, Some(EntryKind::File | EntryKind::Symlink)) {
            if name == ".ignore" {
                self.ignore_file = true;
            } else if name == ".gitignore" {
                self.gitignore_file = true;
            }
        }
        if name == ".git" {
            self.git_dir = true;
            self.repo_marker = true;
        } else if name == ".jj" {
            self.repo_marker = true;
        }
    }
}

fn has_repo_marker(dir: &Path) -> bool {
    dir.join(".git").exists() || dir.join(".jj").exists()
}

fn covers_root(matcher_root: &Path, source: &Path, line: &str, root: &Path) -> bool {
    let mut b = GitignoreBuilder::new(matcher_root);
    b.add_line(Some(source.to_path_buf()), line).is_ok()
        && b.build().is_ok_and(|m| m.matched_path_or_any_parents(root, true).is_ignore())
}

/// Load one ignore file; for an ancestor of the walk root, drop the rules
/// that would hide the root itself.
fn load(matcher_root: &Path, file: &Path, explicit_root: Option<&Path>) -> Option<Gitignore> {
    if !file.is_file() {
        return None;
    }
    let mut b = GitignoreBuilder::new(matcher_root);
    let _ = b.add(file);
    let matcher = b.build().ok().filter(|m| !m.is_empty())?;
    let Some(root) = explicit_root else { return Some(matcher) };
    if !matcher.matched_path_or_any_parents(root, true).is_ignore() {
        return Some(matcher);
    }
    let handle = std::fs::File::open(file).ok()?;
    let mut filtered = GitignoreBuilder::new(matcher_root);
    for (index, line) in std::io::BufReader::new(handle).lines().enumerate() {
        let Ok(line) = line else { break };
        let line = if index == 0 { line.trim_start_matches('\u{feff}').to_string() } else { line };
        if covers_root(matcher_root, file, &line, root) {
            continue;
        }
        let _ = filtered.add_line(Some(file.to_path_buf()), &line);
    }
    filtered.build().ok().filter(|m| !m.is_empty())
}

struct Matcher {
    global: Option<Gitignore>,
    use_gitignore: bool,
}

fn decide(m: Match<&ignore::gitignore::Glob>) -> Option<bool> {
    match m {
        Match::Ignore(_) => Some(true),
        Match::Whitelist(_) => Some(false),
        Match::None => None,
    }
}

impl Matcher {
    fn is_ignored(&self, state: &IgnoreState, path: &Path, is_dir: bool) -> bool {
        if !self.use_gitignore {
            return false;
        }
        let global_applies = state.any_git && self.global.is_some();
        if !state.chain_has_matchers && !global_applies {
            return false;
        }
        let (mut ig, mut gi, mut ex) = (Match::None, Match::None, Match::None);
        let mut saw_git = false;
        if state.chain_has_matchers {
            let mut frame = Some(state);
            while let Some(f) = frame {
                if ig.is_none()
                    && let Some(m) = &f.ignore
                {
                    ig = m.matched(path, is_dir);
                }
                if gi.is_none()
                    && let Some(m) = &f.gitignore
                {
                    gi = m.matched(path, is_dir);
                }
                if state.any_git
                    && !saw_git
                    && ex.is_none()
                    && let Some(m) = &f.git_exclude
                {
                    ex = m.matched(path, is_dir);
                }
                saw_git |= f.has_git;
                frame = f.parent.as_deref();
            }
        }
        if let Some(d) = decide(ig).or_else(|| decide(gi)).or_else(|| decide(ex)) {
            return d;
        }
        if state.any_git
            && let Some(g) = &self.global
        {
            return decide(g.matched(path, is_dir)).unwrap_or(false);
        }
        false
    }
}

/// `None` for FIFOs, sockets and devices, which upstream never lists.
fn kind_of(ft: std::fs::FileType) -> Option<EntryKind> {
    if ft.is_symlink() {
        Some(EntryKind::Symlink)
    } else if ft.is_dir() {
        Some(EntryKind::Dir)
    } else if ft.is_file() {
        Some(EntryKind::File)
    } else {
        None
    }
}

struct Walker<'a, F> {
    opts: WalkOptions,
    follow_links: FollowLinks,
    ancestors: Vec<DirectoryIdentity>,
    matcher: Matcher,
    budget: &'a Budget,
    visit: F,
}

// Fixed pi-walker directory_identity: Unix dev/inode also catches aliases
// whose canonical spelling differs; other platforms use canonical paths.
#[derive(Clone, Debug, Eq, PartialEq)]
enum DirectoryIdentity {
    #[cfg(unix)]
    Unix { dev: u64, ino: u64 },
    #[cfg(not(unix))]
    Generic(PathBuf),
}
fn directory_identity(path: &Path) -> std::io::Result<DirectoryIdentity> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::metadata(path)?;
        Ok(DirectoryIdentity::Unix { dev: metadata.dev(), ino: metadata.ino() })
    }
    #[cfg(not(unix))]
    {
        std::fs::canonicalize(path).map(DirectoryIdentity::Generic)
    }
}

impl<F: FnMut(WalkEntry) -> Visit> Walker<'_, F> {
    /// Returns true when the visitor asked to stop.
    fn dir(
        &mut self,
        dir: &Path,
        rel: &str,
        depth: usize,
        state: &Arc<IgnoreState>,
        is_root: bool,
    ) -> Result<bool, WalkError> {
        let Ok(rd) = std::fs::read_dir(dir) else { return Ok(false) };
        let mut names = EntryNames::default();
        let mut entries: Vec<(OsString, EntryKind)> = Vec::new();
        for e in rd {
            let Ok(e) = e else { continue };
            let Ok(ft) = e.file_type() else { continue };
            let kind = kind_of(ft);
            let name = e.file_name();
            names.record(&name, kind);
            if let Some(kind) = kind {
                entries.push((name, kind));
            }
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        let state =
            if is_root || !self.opts.use_gitignore { Arc::clone(state) } else { IgnoreState::child(dir, state, names) };
        for (name, mut kind) in entries {
            self.budget.check()?;
            let bytes = name.as_encoded_bytes();
            if !self.opts.include_hidden && bytes.first() == Some(&b'.') {
                continue;
            }
            if name == ".git" || (self.opts.skip_node_modules && name == "node_modules") {
                continue;
            }
            let next_depth = depth + 1;
            if next_depth > self.opts.max_depth {
                continue;
            }
            let path = dir.join(&name);
            if kind == EntryKind::Symlink && self.follow_links == FollowLinks::Always {
                let Ok(metadata) = std::fs::metadata(&path) else { continue };
                let Some(target_kind) = kind_of(metadata.file_type()) else { continue };
                kind = target_kind;
            }
            let is_dir = kind == EntryKind::Dir;
            if self.matcher.is_ignored(&state, &path, is_dir) {
                continue;
            }
            let name_str = name.to_string_lossy();
            let relative = if rel.is_empty() { name_str.into_owned() } else { format!("{rel}/{name_str}") };
            // Keep copies only when the directory is descended afterwards.
            let identity = if is_dir && self.follow_links == FollowLinks::Always {
                let Ok(identity) = directory_identity(&path) else { continue };
                if self.ancestors.contains(&identity) {
                    continue;
                }
                Some(identity)
            } else {
                None
            };
            let descend = (is_dir && next_depth < self.opts.max_depth).then(|| (path.clone(), relative.clone()));
            if let Visit::Stop = (self.visit)(WalkEntry { relative, path, kind }) {
                return Ok(true);
            }
            if let Some((path, relative)) = descend {
                if let Some(identity) = identity {
                    self.ancestors.push(identity);
                }
                let stopped = self.dir(&path, &relative, next_depth, &state, false);
                if self.follow_links == FollowLinks::Always {
                    self.ancestors.pop();
                }
                if stopped? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}

/// Visit the entries below `root` (the root itself is not emitted).
pub fn walk(
    root: &Path,
    opts: WalkOptions,
    budget: &Budget,
    visit: impl FnMut(WalkEntry) -> Visit,
) -> Result<(), WalkError> {
    let global = if opts.use_gitignore {
        let (m, _) = Gitignore::global();
        (!m.is_empty()).then_some(m)
    } else {
        None
    };
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let state = IgnoreState::root(&root, opts.use_gitignore);
    let mut w = Walker {
        opts,
        follow_links: FollowLinks::Never,
        ancestors: Vec::new(),
        matcher: Matcher { global, use_gitignore: opts.use_gitignore },
        budget,
        visit,
    };
    w.dir(&root, "", 0, &state, true)?;
    Ok(())
}

/// Same visitor scanner and ignore owner, with explicit collector policies.
fn collect_entries_uncached(
    root: &Path,
    options: CollectOptions,
    budget: &Budget,
) -> Result<CollectedEntries, WalkError> {
    budget.check()?;
    // Inspect the original operand before canonicalization: Never must not
    // silently turn a symlink root into a directory walk.
    let Ok(root_metadata) = std::fs::symlink_metadata(root) else {
        budget.check()?;
        return Ok(CollectedEntries { entries: Vec::new(), cache_age_ms: 0 });
    };
    if root_metadata.is_symlink() && options.follow_links == FollowLinks::Never {
        budget.check()?;
        return Ok(CollectedEntries { entries: Vec::new(), cache_age_ms: 0 });
    }
    let root = cache::resolve_path(root);
    let global = if options.walk.use_gitignore {
        let (matcher, _) = Gitignore::global();
        (!matcher.is_empty()).then_some(matcher)
    } else {
        None
    };
    let state = IgnoreState::root(&root, options.walk.use_gitignore);
    let mut entries = Vec::new();
    let visit = |entry: WalkEntry| {
        let metadata = if options.detail == WalkDetail::Full {
            if options.follow_links == FollowLinks::Always {
                std::fs::metadata(&entry.path).ok()
            } else {
                std::fs::symlink_metadata(&entry.path).ok()
            }
        } else {
            None
        };
        let modified = metadata
            .as_ref()
            .and_then(|value| value.modified().ok())
            .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
            .map(|value| value.as_millis() as f64);
        let size = metadata.as_ref().filter(|_| entry.kind == EntryKind::File).map(std::fs::Metadata::len);
        entries.push(CollectedEntry {
            relative: entry.relative,
            path: entry.path,
            kind: entry.kind,
            mtime_ms: modified,
            size,
        });
        Visit::Continue
    };
    let ancestors = if options.follow_links == FollowLinks::Always {
        directory_identity(&root).into_iter().collect()
    } else {
        Vec::new()
    };
    let mut walker = Walker {
        opts: options.walk,
        follow_links: options.follow_links,
        ancestors,
        matcher: Matcher { global, use_gitignore: options.walk.use_gitignore },
        budget,
        visit,
    };
    walker.dir(&root, "", 0, &state, true)?;
    drop(walker);
    budget.check()?;
    entries.sort_by(|left, right| left.relative.cmp(&right.relative));
    budget.check()?;
    Ok(CollectedEntries { entries, cache_age_ms: 0 })
}
