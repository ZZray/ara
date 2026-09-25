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
//! Differences: sequential only (no parallel walker, scan cache or
//! platform-specific directory reads). Unreadable directories are skipped.
//! Upstream grep does not descend a symlinked directory root (its walker
//! lstats the root and finds a symlink), so it reports no matches there; ARA
//! walks the resolved directory for grep as upstream glob does.

use crate::engine::{Budget, EngineError};
use ignore::Match;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::ffi::{OsStr, OsString};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

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

#[derive(Clone, Copy, Debug)]
pub struct WalkOptions {
    pub include_hidden: bool,
    pub use_gitignore: bool,
    pub skip_node_modules: bool,
    /// Deepest entry depth visited (root children are depth 1).
    pub max_depth: usize,
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
    matcher: Matcher,
    budget: &'a Budget,
    visit: F,
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
    ) -> Result<bool, EngineError> {
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
        for (name, kind) in entries {
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
            let is_dir = kind == EntryKind::Dir;
            if self.matcher.is_ignored(&state, &path, is_dir) {
                continue;
            }
            let name_str = name.to_string_lossy();
            let relative = if rel.is_empty() { name_str.into_owned() } else { format!("{rel}/{name_str}") };
            // Keep copies only when the directory is descended afterwards.
            let descend = (is_dir && next_depth < self.opts.max_depth).then(|| (path.clone(), relative.clone()));
            if let Visit::Stop = (self.visit)(WalkEntry { relative, path, kind }) {
                return Ok(true);
            }
            if let Some((path, relative)) = descend
                && self.dir(&path, &relative, next_depth, &state, false)?
            {
                return Ok(true);
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
) -> Result<(), EngineError> {
    let global = if opts.use_gitignore {
        let (m, _) = Gitignore::global();
        (!m.is_empty()).then_some(m)
    } else {
        None
    };
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let state = IgnoreState::root(&root, opts.use_gitignore);
    let mut w = Walker { opts, matcher: Matcher { global, use_gitignore: opts.use_gitignore }, budget, visit };
    w.dir(&root, "", 0, &state, true)?;
    Ok(())
}
