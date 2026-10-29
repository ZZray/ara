//! Fixed worktree.ts Git algorithms. All mutating operations use the vendored Git facade.
use anyhow::{Context, Result, bail};
use ara_task_vcs::{
    git::GitRepo,
    types::{ApplyOptions, CommitAuthor, CommitOptions, DiffOptions},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, HashMap},
    fs,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

pub const BASELINE_MAX_CONTENT_BYTES: usize = 1024 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RepoBaseline {
    pub repo_root: PathBuf,
    pub head_commit: String,
    pub staged: String,
    pub unstaged: String,
    pub untracked: Vec<String>,
    pub untracked_patch: String,
}
impl RepoBaseline {
    fn patches(&self) -> [&str; 3] {
        [&self.staged, &self.unstaged, &self.untracked_patch]
    }
    fn has_wip(&self) -> bool {
        self.patches().iter().any(|p| !p.trim().is_empty())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NestedBaseline {
    pub relative_path: PathBuf,
    pub baseline: RepoBaseline,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorktreeBaseline {
    pub root: RepoBaseline,
    pub nested: Vec<NestedBaseline>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NestedPatch {
    pub relative_path: PathBuf,
    pub patch: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Delta {
    pub root_patch: String,
    pub nested_patches: Vec<NestedPatch>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BranchResult {
    pub branch_name: Option<String>,
    pub base_sha: String,
    pub tip: Option<String>,
    pub rescue_ref: Option<String>,
}

pub fn repo_root(cwd: &Path) -> Result<PathBuf> {
    if ara_task_vcs::is_pure_jj(cwd)? {
        bail!(
            "Isolated task execution requires a Git checkout; pure Jujutsu workspace. Run `jj git init --colocate` or disable task isolation."
        );
    }
    Ok(GitRepo::require(cwd)?.root().to_path_buf())
}

pub fn safe_relative(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        bail!("unsafe repository-relative path: {}", path.display());
    }
    Ok(())
}

/// Same exclusions and nested boundary as fixed OMP. Never walk directory symlinks.
pub fn discover_nested(root: &Path) -> Result<Vec<PathBuf>> {
    let submodules: BTreeSet<_> = GitRepo::require(root)?
        .submodule_paths()?
        .into_iter()
        .map(PathBuf::from)
        .collect();
    fn walk(
        root: &Path,
        dir: &Path,
        skip: &BTreeSet<PathBuf>,
        out: &mut Vec<PathBuf>,
    ) -> Result<()> {
        let Ok(entries) = fs::read_dir(dir) else {
            return Ok(());
        };
        for entry in entries {
            let entry = entry?;
            if entry.file_name() == ".git"
                || entry.file_name() == "node_modules"
                || !entry.file_type()?.is_dir()
            {
                continue;
            }
            let path = entry.path();
            let rel = path.strip_prefix(root)?.to_path_buf();
            if path.join(".git").symlink_metadata().is_ok() && !skip.contains(&rel) {
                out.push(rel);
                continue;
            }
            walk(root, &path, skip, out)?;
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(root, root, &submodules, &mut out)?;
    out.sort();
    Ok(out)
}

fn untracked_patch(repo: &GitRepo, paths: &[String]) -> Result<String> {
    let mut parts = Vec::new();
    for path in paths {
        safe_relative(Path::new(path))?;
        #[cfg(windows)]
        let null = Path::new("NUL");
        #[cfg(not(windows))]
        let null = Path::new("/dev/null");
        // Fixed source tolerates an entry disappearing between listing and encoding.
        match repo.diff_no_index(null, Path::new(path), true) {
            Ok(patch) if !patch.trim().is_empty() => parts.push(patch),
            Ok(_) => {}
            // Source tolerates native VCS encoding errors (including nested-repo directory entries).
            Err(_) => {}
        }
    }
    Ok(parts.join("\n"))
}

pub fn capture_repo_baseline(root: &Path, budget: usize) -> Result<RepoBaseline> {
    let repo = GitRepo::require(root)?;
    let staged = repo.diff_text(&DiffOptions {
        binary: true,
        cached: true,
        max_bytes: Some(budget),
        ..Default::default()
    })?;
    let unstaged = repo.diff_text(&DiffOptions {
        binary: true,
        max_bytes: Some(budget.saturating_sub(staged.len())),
        ..Default::default()
    })?;
    let untracked = repo.ls_files(true, true)?;
    let mut bytes = staged
        .len()
        .checked_add(unstaged.len())
        .context("baseline byte overflow")?;
    for path in &untracked {
        safe_relative(Path::new(path))?;
        if let Ok(meta) = fs::symlink_metadata(root.join(path)) {
            if meta.is_file() {
                bytes = bytes
                    .checked_add(usize::try_from(meta.len())?)
                    .context("baseline byte overflow")?;
            }
        }
        if bytes > budget {
            bail!(
                "Working tree at {} exceeds the {} byte isolation-snapshot budget; commit or gitignore bulk content, or disable task isolation",
                root.display(),
                budget
            );
        }
    }
    let untracked_patch = untracked_patch(&repo, &untracked)?;
    Ok(RepoBaseline {
        repo_root: root.to_path_buf(),
        head_commit: repo.head_sha()?.unwrap_or_default(),
        staged,
        unstaged,
        untracked,
        untracked_patch,
    })
}

/// Read the materialized point-in-time copy, never a second snapshot of the live parent.
pub fn capture_baseline(copy: &Path, parent: &Path, budget: usize) -> Result<WorktreeBaseline> {
    let mut root = capture_repo_baseline(copy, budget)?;
    root.repo_root = parent.to_path_buf();
    let mut nested = Vec::new();
    for relative_path in discover_nested(copy)? {
        let mut baseline = capture_repo_baseline(&copy.join(&relative_path), budget)?;
        baseline.repo_root = parent.join(&relative_path);
        nested.push(NestedBaseline {
            relative_path,
            baseline,
        });
    }
    Ok(WorktreeBaseline { root, nested })
}

/// Temporary indexes are local to the lease/artifact workspace, never the process temp root.
pub fn synthetic_tree(
    root: &Path,
    base: &str,
    patches: &[&str],
    three_way: bool,
    scratch: &Path,
) -> Result<String> {
    fs::create_dir_all(scratch)?;
    let index = scratch.join(format!("i-{}", uuid::Uuid::new_v4().simple()));
    let repo = GitRepo::require(root)?;
    let result = (|| {
        repo.read_tree(base, Some(&index))?;
        for patch in patches {
            if !patch.trim().is_empty() {
                repo.apply_patch(
                    patch,
                    &ApplyOptions {
                        cached: true,
                        index_path: Some(index.clone()),
                        three_way,
                        ..Default::default()
                    },
                )?;
            }
        }
        Ok(repo.write_tree(Some(&index))?)
    })();
    let _ = fs::remove_file(index);
    result
}

fn diff_tree(repo: &GitRepo, base: &str, head: &str) -> Result<String> {
    if base.is_empty() || head.is_empty() {
        return Ok(String::new());
    }
    Ok(repo.diff_tree(base, head, true)?)
}

pub fn capture_repo_delta(copy: &Path, baseline: &RepoBaseline, scratch: &Path) -> Result<String> {
    let repo = GitRepo::require(copy)?;
    // Render current changes under the same source budget, including stat-before-read untracked.
    let current = capture_repo_baseline(copy, BASELINE_MAX_CONTENT_BYTES)?;
    let committed = if current.head_commit != baseline.head_commit {
        diff_tree(&repo, &baseline.head_commit, &current.head_commit)?
    } else {
        String::new()
    };
    // Fixed OMP seeds dirty-side blobs in the destination ODB so branch --3way
    // can subtract baseline WIP. Temporary indexes leave parent refs and WIP intact.
    let object_root = &baseline.repo_root;
    let object_repo = GitRepo::require(object_root)?;
    let before = synthetic_tree(
        object_root,
        &baseline.head_commit,
        &baseline.patches(),
        false,
        scratch,
    )?;
    let after = synthetic_tree(
        object_root,
        &baseline.head_commit,
        &[
            &committed,
            &current.staged,
            &current.unstaged,
            &current.untracked_patch,
        ],
        false,
        scratch,
    )?;
    diff_tree(&object_repo, &before, &after)
}

pub fn capture_delta(copy: &Path, baseline: &WorktreeBaseline, scratch: &Path) -> Result<Delta> {
    let root_patch = capture_repo_delta(copy, &baseline.root, scratch)?;
    let mut nested_patches = Vec::new();
    for nested in &baseline.nested {
        let path = copy.join(&nested.relative_path);
        if path.join(".git").symlink_metadata().is_err() {
            continue;
        }
        let patch = capture_repo_delta(&path, &nested.baseline, scratch)?;
        if !patch.trim().is_empty() {
            nested_patches.push(NestedPatch {
                relative_path: nested.relative_path.clone(),
                patch,
            });
        }
    }
    Ok(Delta {
        root_patch,
        nested_patches,
    })
}

// Parse quoted Git C strings, including octal escapes used for non-ASCII paths.
fn unquote(raw: &str) -> String {
    if !raw.starts_with('"') {
        return raw.to_owned();
    }
    let mut bytes = Vec::new();
    let mut chars = raw.as_bytes()[1..raw.len().saturating_sub(1)]
        .iter()
        .copied()
        .peekable();
    while let Some(c) = chars.next() {
        if c != b'\\' {
            bytes.push(c);
            continue;
        }
        match chars.next() {
            Some(b'n') => bytes.push(b'\n'),
            Some(b't') => bytes.push(b'\t'),
            Some(b'r') => bytes.push(b'\r'),
            Some(c @ b'0'..=b'7') => {
                let mut v = c - b'0';
                for _ in 0..2 {
                    if let Some(d @ b'0'..=b'7') = chars.peek().copied() {
                        chars.next();
                        v = v.wrapping_mul(8).wrapping_add(d - b'0');
                    }
                }
                bytes.push(v);
            }
            Some(c) => bytes.push(c),
            None => {}
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn line_paths(line: &str) -> Vec<String> {
    let Some(rest) = line.strip_prefix("diff --git ") else {
        return Vec::new();
    };
    let mut fields = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;
    for (i, c) in rest.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' && quoted {
            escaped = true;
            continue;
        }
        if c == '"' {
            quoted = !quoted;
        }
        if c == ' ' && !quoted {
            fields.push(&rest[start..i]);
            start = i + 1;
        }
    }
    fields.push(&rest[start..]);
    fields
        .into_iter()
        .take(2)
        .map(unquote)
        .filter_map(|s| {
            s.strip_prefix("a/")
                .or_else(|| s.strip_prefix("b/"))
                .map(str::to_owned)
        })
        .collect()
}
pub fn touched_files(patch: &str) -> Vec<String> {
    patch
        .lines()
        .flat_map(line_paths)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}
fn filter_patch(patch: &str, paths: &BTreeSet<String>) -> String {
    let mut result = String::new();
    let mut selected = false;
    for line in patch.split_inclusive('\n') {
        if line.starts_with("diff --git ") {
            selected = line_paths(line.trim_end())
                .iter()
                .any(|p| paths.contains(p));
        }
        if selected {
            result.push_str(line);
        }
    }
    result
}

fn patch_tree(
    root: &Path,
    parent: &str,
    patch: &str,
    wip: Option<&RepoBaseline>,
    scratch: &Path,
) -> Result<String> {
    if let Ok(tree) = synthetic_tree(root, parent, &[patch], false, scratch) {
        return Ok(tree);
    }
    if let Ok(tree) = synthetic_tree(root, parent, &[patch], true, scratch) {
        return Ok(tree);
    }
    let wip = wip.context("plain and three-way task delta application failed")?;
    let with_wip = synthetic_tree(root, parent, &wip.patches(), false, scratch)?;
    let delta_files: BTreeSet<_> = touched_files(patch).into_iter().collect();
    let only: BTreeSet<_> = wip
        .patches()
        .iter()
        .flat_map(|p| touched_files(p))
        .filter(|p| !delta_files.contains(p))
        .collect();
    let rewind = filter_patch(
        &GitRepo::require(root)?.diff_tree(&with_wip, parent, true)?,
        &only,
    );
    synthetic_tree(
        root,
        parent,
        &[
            &wip.staged,
            &wip.unstaged,
            &wip.untracked_patch,
            patch,
            &rewind,
        ],
        false,
        scratch,
    )
}

fn commit_patch(
    root: &Path,
    parent: &str,
    patch: &str,
    message: &str,
    author: Option<&CommitAuthor>,
    wip: Option<&RepoBaseline>,
    scratch: &Path,
) -> Result<String> {
    let tree = patch_tree(root, parent, patch, wip, scratch)?;
    let parents = if parent.is_empty() {
        vec![]
    } else {
        vec![parent.to_owned()]
    };
    Ok(GitRepo::require(root)?.commit_tree(&tree, &parents, message, author)?)
}

/// Gate identity follows the canonical Git common directory, shared by linked worktrees.
pub fn repo_gate(root: &Path) -> Result<Arc<tokio::sync::Mutex<()>>> {
    static GATES: OnceLock<Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    let common = fs::canonicalize(&GitRepo::require(root)?.info().common_dir)?;
    let mut gates = GATES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    Ok(gates.entry(common).or_default().clone())
}

#[derive(Clone)]
struct BranchOwner {
    resource: String,
    expected: Option<String>,
}
fn branch_owners() -> &'static Mutex<HashMap<(PathBuf, String), BranchOwner>> {
    static OWNERS: OnceLock<Mutex<HashMap<(PathBuf, String), BranchOwner>>> = OnceLock::new();
    OWNERS.get_or_init(Default::default)
}

/// Caller holds canonical repo_gate. Public names retain OMP identity; private rescue refs are unique.
pub fn commit_to_branch(
    copy: &Path,
    baseline: &WorktreeBaseline,
    task: &str,
    resource: &str,
    description: Option<&str>,
    delta: &Delta,
    scratch: &Path,
) -> Result<BranchResult> {
    let root = &baseline.root.repo_root;
    let repo = GitRepo::require(root)?;
    let isolated = GitRepo::require(copy)?;
    let base = &baseline.root.head_commit;
    let branch = format!("omp/task/{task}");
    let name = format!("refs/heads/{branch}");
    if delta.root_patch.trim().is_empty() {
        return Ok(BranchResult {
            branch_name: None,
            base_sha: base.clone(),
            tip: None,
            rescue_ref: None,
        });
    }
    let key = (fs::canonicalize(&repo.info().common_dir)?, name.clone());
    let expected = {
        let mut owners = branch_owners().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(owner) = owners.get(&key) {
            if owner.resource != resource {
                bail!(
                    "public task branch is owned by another ResourceId; retained workspace: {}",
                    copy.display()
                );
            }
            owner.expected.clone()
        } else {
            if repo.resolve_ref(&name)?.is_some() {
                bail!(
                    "public task branch already exists without this live owner; retained workspace: {}",
                    copy.display()
                );
            }
            owners.insert(
                key.clone(),
                BranchOwner {
                    resource: resource.into(),
                    expected: None,
                },
            );
            None
        }
    };
    let rescue = format!(
        "refs/ara/task-rescue/{resource}/{}",
        uuid::Uuid::new_v4().simple()
    );
    let head = isolated.head_sha()?.unwrap_or_default();
    let message = description.unwrap_or(task);
    let mut tip = base.clone();
    if !head.is_empty() && head != *base && !baseline.root.has_wip() {
        let source_url = reqwest::Url::from_file_path(copy).map_err(|_| {
            anyhow::anyhow!("cannot encode isolated Git source path {}", copy.display())
        })?;
        tokio::runtime::Handle::current().block_on(repo.fetch(
            source_url.as_str(),
            "HEAD",
            &rescue,
            Some(std::time::Duration::from_secs(1800)),
            None,
        ))?;
        tip = head.clone();
        let clean = RepoBaseline {
            repo_root: copy.to_path_buf(),
            head_commit: head.clone(),
            staged: String::new(),
            unstaged: String::new(),
            untracked: vec![],
            untracked_patch: String::new(),
        };
        let leftover = capture_repo_delta(copy, &clean, scratch)?;
        if !leftover.trim().is_empty() {
            tip = commit_patch(root, &tip, &leftover, message, None, None, scratch)?;
        }
    } else if !head.is_empty() && head != *base && baseline.root.has_wip() {
        // Seed dirty-side blobs in parent before filtering each original commit.
        synthetic_tree(root, base, &baseline.root.patches(), false, scratch)?;
        let dirty = synthetic_tree(copy, base, &baseline.root.patches(), false, scratch)?;
        let mut previous = base.clone();
        for sha in isolated.rev_list_range(base, &head)? {
            let state = isolated.diff_tree(&dirty, &format!("{sha}^{{tree}}"), true)?;
            let filtered = synthetic_tree(root, base, &[&state], true, scratch)?;
            let part = repo.diff_tree(&previous, &filtered, true)?;
            if !part.trim().is_empty() {
                let details = isolated.commit_details(&sha)?;
                tip = commit_patch(
                    root,
                    &tip,
                    &part,
                    &details.message,
                    Some(&details.author),
                    None,
                    scratch,
                )?;
            }
            previous = filtered;
            // Preserve every completed prefix before the next potentially failing step.
            let old = repo.resolve_ref(&rescue)?;
            if !repo.compare_and_swap_ref(&rescue, old.as_deref(), Some(&tip))? {
                bail!("rescue reference changed concurrently: {rescue}");
            }
        }
        if tip == *base {
            tip = commit_patch(
                root,
                base,
                &delta.root_patch,
                message,
                None,
                Some(&baseline.root),
                scratch,
            )?;
        } else {
            let final_tree = synthetic_tree(root, base, &[&delta.root_patch], true, scratch)?;
            let leftover = repo.diff_tree(&previous, &final_tree, true)?;
            if !leftover.trim().is_empty() {
                tip = commit_patch(root, &tip, &leftover, message, None, None, scratch)?;
            }
        }
    } else {
        tip = commit_patch(
            root,
            base,
            &delta.root_patch,
            message,
            None,
            baseline.root.has_wip().then_some(&baseline.root),
            scratch,
        )?;
    }
    let old_rescue = repo.resolve_ref(&rescue)?;
    if !repo.compare_and_swap_ref(&rescue, old_rescue.as_deref(), Some(&tip))? {
        bail!("rescue ref CAS conflict: {rescue}");
    }
    if !repo.compare_and_swap_ref(&name, expected.as_deref(), Some(&tip))? {
        bail!(
            "public branch CAS conflict; recovery ref {rescue}; retained workspace {}",
            copy.display()
        );
    }
    branch_owners()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(
            key,
            BranchOwner {
                resource: resource.into(),
                expected: Some(tip.clone()),
            },
        );
    Ok(BranchResult {
        branch_name: Some(branch),
        base_sha: base.clone(),
        tip: Some(tip),
        rescue_ref: Some(rescue),
    })
}

/// Only delete a ref if both process-local owner and actual expected OID still agree.
pub fn cleanup_branch(root: &Path, resource: &str, branch: &BranchResult) -> Result<bool> {
    let Some(branch_name) = &branch.branch_name else {
        return Ok(true);
    };
    let repo = GitRepo::require(root)?;
    let name = format!("refs/heads/{branch_name}");
    let key = (fs::canonicalize(&repo.info().common_dir)?, name.clone());
    let mut owners = branch_owners().lock().unwrap_or_else(|e| e.into_inner());
    let Some(owner) = owners.get(&key) else {
        return Ok(false);
    };
    if owner.resource != resource || owner.expected != branch.tip {
        return Ok(false);
    }
    let removed = repo.compare_and_swap_ref(&name, owner.expected.as_deref(), None)?;
    if removed {
        owners.remove(&key);
    }
    Ok(removed)
}

pub fn apply_nested(root: &Path, patches: &[NestedPatch], resource: &str) -> Result<Vec<String>> {
    let mut warnings = Vec::new();
    for patch in patches {
        safe_relative(&patch.relative_path)?;
        let path = root.join(&patch.relative_path);
        if path.join(".git").symlink_metadata().is_err() {
            continue;
        }
        let repo = GitRepo::require(&path)?;
        let own_stash = if repo.is_dirty()? {
            if repo.stash_push(Some(&format!("ara-isolation-{resource}")))? {
                repo.resolve_ref("refs/stash")?
            } else {
                None
            }
        } else {
            None
        };
        let applied = (|| {
            repo.apply_patch(&patch.patch, &ApplyOptions::default())?;
            if repo.is_dirty()? {
                let files = touched_files(&patch.patch);
                if files.is_empty() {
                    bail!("nested patch has no stageable paths");
                }
                for file in &files {
                    safe_relative(Path::new(file))?;
                }
                repo.stage_files(&files)?;
                repo.commit_create("changes from isolated task(s)", &CommitOptions::default())?;
            }
            Ok(())
        })();
        if let Some(sha) = own_stash {
            // The canonical Host repo gate serializes in-process stash work; only pop while our
            // captured entry is still on top. External Git can race this check and the upstream
            // top-only stash_try_pop call; that inherited window is not an external-process lock.
            if repo.resolve_ref("refs/stash")?.as_deref() != Some(&sha)
                || !repo.stash_try_pop(true).unwrap_or(false)
            {
                warnings.push(format!(
                    "Nested {} WIP stash {sha} could not be restored; entry preserved",
                    patch.relative_path.display()
                ));
            }
        }
        applied?;
    }
    Ok(warnings)
}
