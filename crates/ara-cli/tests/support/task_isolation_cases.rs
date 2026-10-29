use crate::task_isolation::{IsolationLease, IsolationRequest, worktree};
use ara_task_vcs::{
    git::GitRepo,
    types::{CommitOptions, DiffOptions},
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};

struct Fixture {
    _temp: RetainedRoot,
    root: PathBuf,
    resources: PathBuf,
    artifacts: PathBuf,
}
/// Independent EXEC inspects original files/repos after the bank; success and panic both retain them.
struct RetainedRoot(PathBuf);
impl RetainedRoot {
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for RetainedRoot {
    fn drop(&mut self) {
        fn observe(root: &Path, args: &[&str]) -> serde_json::Value {
            match Command::new("git")
                .current_dir(root)
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .env_remove("GIT_INDEX_FILE")
                .env("GIT_OPTIONAL_LOCKS", "0")
                .args(args)
                .output()
            {
                Ok(output) => {
                    serde_json::json!({"args":args,"success":output.status.success(),"code":output.status.code(),"stdout":String::from_utf8_lossy(&output.stdout),"stderr":String::from_utf8_lossy(&output.stderr)})
                }
                Err(error) => {
                    serde_json::json!({"args":args,"observation_error":error.to_string()})
                }
            }
        }
        fn inventory(root: &Path, path: &Path, out: &mut Vec<serde_json::Value>) {
            let Ok(entries) = fs::read_dir(path) else {
                return;
            };
            for entry in entries.flatten() {
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                let path = entry.path();
                if kind.is_dir() {
                    inventory(root, &path, out);
                } else {
                    out.push(serde_json::json!({"path":path.strip_prefix(root).ok(),"symlink":kind.is_symlink(),"bytes":entry.metadata().ok().map(|m|m.len())}));
                }
            }
        }
        let parent = self.0.join("parent");
        let resources = self.0.join("resources");
        let artifacts = self.0.join("artifacts");
        let mut files = Vec::new();
        inventory(&self.0, &artifacts, &mut files);
        let resource_observations:Vec<_>=fs::read_dir(&resources).into_iter().flatten().flatten().map(|entry|{
            let leaf=entry.path();let cwd=leaf.join("m");serde_json::json!({"leaf":leaf,"cwd":cwd,"cwd_exists":cwd.exists(),"owner":fs::read(leaf.join("owner.json")).ok().and_then(|bytes|serde_json::from_slice::<serde_json::Value>(&bytes).ok())})
        }).collect();
        let evidence = serde_json::json!({"state":"ORIGINAL_PRIVATE_FIXTURE_RETAINED_FOR_INDEPENDENT_EXEC","root":self.0,"panicking":std::thread::panicking(),"test_process_id":std::process::id(),
            "boundary":"actual private Git/PAL/resource receipts; this bank does not stand for Native ChildHost or real-model acceptance", "parent":parent,
            "git":[observe(&parent,&["rev-parse","HEAD"]),observe(&parent,&["status","--porcelain=v1","-z"]),observe(&parent,&["show-ref"]),observe(&parent,&["diff","--binary","--no-ext-diff"]),observe(&parent,&["diff","--binary","--no-ext-diff","--cached"])],
            "resources_at_fixture_exit":resource_observations,"durable_artifact_files":files});
        let _ = fs::write(
            self.0.join("fixture-exit-evidence.json"),
            serde_json::to_vec_pretty(&evidence).unwrap_or_default(),
        );
    }
}
impl Fixture {
    fn new(label: &str) -> Self {
        let base = std::env::var_os("ARA_TEST_PEER_ARTIFACT_DIR")
            .map(|root| PathBuf::from(root).join("task-isolation-primitives"))
            .or_else(|| std::env::var_os("ARA_TASK_ISOLATION_TEST_ROOT").map(PathBuf::from))
            .unwrap_or_else(std::env::temp_dir);
        fs::create_dir_all(&base).unwrap();
        let temp = tempfile::Builder::new()
            .prefix(&format!("task-iso-{label}-"))
            .tempdir_in(base)
            .unwrap();
        let temp = RetainedRoot(temp.keep());
        let root = temp.path().join("parent");
        fs::create_dir(&root).unwrap();
        init(&root);
        fs::write(
            root.join("tracked.txt"),
            "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\n",
        )
        .unwrap();
        fs::write(root.join("other.txt"), "original\n").unwrap();
        fs::write(root.join("binary.bin"), [0, 1, 2, 0, 255]).unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-qm", "baseline"]);
        let resources = temp.path().join("resources");
        let artifacts = temp.path().join("artifacts");
        Self {
            _temp: temp,
            root,
            resources,
            artifacts,
        }
    }
    fn request(&self, task: &str, mode: &str) -> IsolationRequest {
        IsolationRequest {
            repo_root: self.root.clone(),
            resource_root: self.resources.clone(),
            artifacts_dir: self.artifacts.clone(),
            task_id: task.into(),
            merge_mode: mode.into(),
            backend: "rcopy".into(),
            description: Some("isolated agent changes".into()),
        }
    }
    async fn lease(&self, task: &str, mode: &str) -> Arc<IsolationLease> {
        IsolationLease::create(self.request(task, mode))
            .await
            .unwrap()
    }
    fn staged(&self) -> Vec<u8> {
        fs::read(self.root.join(".git/index")).unwrap()
    }
}
fn git(root: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(root)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "private Git {:?}: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim_end().to_owned()
}
fn init(root: &Path) {
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["config", "user.name", "Task fixture"]);
    git(
        root,
        &["config", "user.email", "task-fixture@example.invalid"],
    );
    git(root, &["config", "core.autocrlf", "false"]);
}
fn peer(generation: u64) -> crate::host_peers::PeerKey {
    crate::host_peers::PeerKey {
        root: "private-root".into(),
        id: "same-task".into(),
        generation,
    }
}

/// Fixed OMP worktree.test.ts:692-825, exercised inside the existing G1 bank.
/// These are verbatim metadata-copy shapes, before Host lease preparation.
fn detach_metadata_parity_cases() {
    use ara_task_vcs::{git::detach_git_dir, types::DetachGitDirResult};
    fn linked(f: &Fixture, name: &str) -> PathBuf {
        let path = f._temp.path().join(name);
        git(
            &f.root,
            &["worktree", "add", "-q", path.to_str().unwrap(), "-b", name],
        );
        path
    }
    fn copy(source: &Path, target: &Path) {
        fs::create_dir(target).unwrap();
        for entry in fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let destination = target.join(entry.file_name());
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                copy(&entry.path(), &destination);
            } else {
                assert!(
                    kind.is_file(),
                    "metadata fixture contains no symlink entries"
                );
                fs::copy(entry.path(), destination).unwrap();
            }
        }
    }
    fn common(path: &Path) -> PathBuf {
        GitRepo::require(path).unwrap().info().common_dir.clone()
    }
    let f = Fixture::new("detach-unborn");
    let parent = linked(&f, "unborn-parent");
    git(&parent, &["checkout", "--orphan", "fresh-orphan"]);
    git(&parent, &["rm", "-rf", "--cached", "."]);
    for name in ["tracked.txt", "other.txt", "binary.bin"] {
        fs::remove_file(parent.join(name)).unwrap();
    }
    fs::write(parent.join("staged.txt"), "staged\n").unwrap();
    git(&parent, &["add", "staged.txt"]);
    let copied = f._temp.path().join("copied-unborn");
    copy(&parent, &copied);
    let before = git(&copied, &["status", "--porcelain=v1"]);
    let source_common = common(&parent);
    assert_eq!(
        detach_git_dir(&copied, &source_common).unwrap(),
        DetachGitDirResult::Detached
    );
    assert_eq!(
        git(&copied, &["symbolic-ref", "HEAD"]),
        "refs/heads/fresh-orphan"
    );
    assert_ne!(
        fs::canonicalize(common(&copied)).unwrap(),
        fs::canonicalize(&source_common).unwrap()
    );
    assert_eq!(git(&copied, &["status", "--porcelain=v1"]), before);
    git(&copied, &["checkout", "-q", "-b", "feature/task-unborn"]);
    fs::write(copied.join("agent.txt"), "first task commit\n").unwrap();
    git(&copied, &["add", "agent.txt"]);
    git(&copied, &["commit", "-qm", "first task commit"]);
    assert_eq!(
        git(&parent, &["symbolic-ref", "HEAD"]),
        "refs/heads/fresh-orphan"
    );
    assert!(
        !git(&parent, &["branch", "--format=%(refname:short)"]).contains("feature/task-unborn")
    );

    let f = Fixture::new("detach-sparse");
    for name in ["keep", "drop"] {
        fs::create_dir(f.root.join(name)).unwrap();
        fs::write(f.root.join(name).join("file.txt"), format!("{name}\n")).unwrap();
    }
    git(&f.root, &["add", "keep", "drop"]);
    git(&f.root, &["commit", "-qm", "keep and drop"]);
    let parent = linked(&f, "sparse-parent");
    git(&parent, &["sparse-checkout", "init", "--cone"]);
    git(&parent, &["sparse-checkout", "set", "keep"]);
    assert_eq!(git(&parent, &["status", "--porcelain=v1"]), "");
    assert!(!parent.join("drop/file.txt").exists());
    let copied = f._temp.path().join("copied-sparse");
    copy(&parent, &copied);
    assert_eq!(
        detach_git_dir(&copied, &common(&parent)).unwrap(),
        DetachGitDirResult::Detached
    );
    assert_eq!(git(&copied, &["status", "--porcelain=v1"]), "");
    assert_eq!(
        git(&copied, &["ls-files", "-t", "drop/file.txt"]),
        "S drop/file.txt"
    );
    assert_eq!(git(&copied, &["config", "core.sparseCheckout"]), "true");
    assert!(!copied.join("drop/file.txt").exists());

    let f = Fixture::new("detach-shallow-split");
    git(&f.root, &["config", "core.fsmonitor", "false"]);
    fs::write(f.root.join("second.txt"), "second\n").unwrap();
    git(&f.root, &["add", "second.txt"]);
    git(&f.root, &["commit", "-qm", "second commit"]);
    let shallow = f._temp.path().join("shallow");
    let source_path = f.root.to_string_lossy().replace('\\', "/");
    let source_url = format!(
        "file://{}{}",
        if source_path.starts_with('/') {
            ""
        } else {
            "/"
        },
        source_path
    );
    git(
        &f.root,
        &[
            "-c",
            "core.fsmonitor=false",
            "clone",
            "-q",
            "--depth",
            "1",
            &source_url,
            shallow.to_str().unwrap(),
        ],
    );
    git(&shallow, &["config", "core.fileMode", "false"]);
    git(&shallow, &["config", "core.fsmonitor", "false"]);
    git(&shallow, &["config", "core.splitIndex", "true"]);
    let parent = f._temp.path().join("shallow-parent");
    git(
        &shallow,
        &[
            "worktree",
            "add",
            "-q",
            parent.to_str().unwrap(),
            "-b",
            "shallow-parent",
            "HEAD",
        ],
    );
    git(
        &parent,
        &[
            "-c",
            "core.fsmonitor=false",
            "update-index",
            "--split-index",
        ],
    );
    let parent_git = GitRepo::require(&parent).unwrap().info().git_dir.clone();
    assert!(
        fs::read_dir(&parent_git).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("sharedindex.")),
        "fixture has an actual split index"
    );
    let copied = f._temp.path().join("copied-shallow");
    copy(&parent, &copied);
    assert_eq!(
        detach_git_dir(&copied, &common(&parent)).unwrap(),
        DetachGitDirResult::Detached
    );
    assert_eq!(git(&copied, &["config", "core.fileMode"]), "false");
    git(&copied, &["config", "core.fsmonitor", "false"]);
    assert_eq!(git(&copied, &["status", "--porcelain=v1"]), "");
    assert!(copied.join(".git/shallow").is_file());
    assert!(fs::read_dir(copied.join(".git")).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("sharedindex.")
    }));
    assert_eq!(git(&copied, &["rev-list", "HEAD"]).lines().count(), 1);

    let f = Fixture::new("detach-source-common-symlink");
    #[cfg(unix)]
    {
        let parent = linked(&f, "alias-parent");
        let base = git(&parent, &["rev-parse", "HEAD"]);
        let alias = f._temp.path().join("main-link");
        std::os::unix::fs::symlink(&f.root, &alias).unwrap();
        let copied = f._temp.path().join("copied-alias");
        copy(&parent, &copied);
        assert_eq!(
            detach_git_dir(&copied, &alias.join(".git")).unwrap(),
            DetachGitDirResult::Detached
        );
        git(
            &copied,
            &["checkout", "-q", "-b", "feature/task-alias", &base],
        );
        fs::write(copied.join("agent.txt"), "private alias task\n").unwrap();
        git(&copied, &["add", "agent.txt"]);
        git(&copied, &["commit", "-qm", "private alias task"]);
        assert_eq!(
            git(&parent, &["rev-parse", "--abbrev-ref", "HEAD"]),
            "alias-parent"
        );
        assert!(
            !git(&parent, &["branch", "--format=%(refname:short)"]).contains("feature/task-alias")
        );
        assert_ne!(
            fs::canonicalize(common(&copied)).unwrap(),
            fs::canonicalize(common(&parent)).unwrap()
        );
    }
    #[cfg(not(unix))]
    fs::write(f._temp.path().join("source-common-symlink-NOT_EXECUTED.json"), br#"{"status":"NOT_EXECUTED","scope":"fixed worktree.test.ts:802-825 sourceCommonDir symlink","reason":"Unix symlink fixture is cfg(unix); Windows symlink privilege and native-path behavior remain unverified"}"#).unwrap();
}

pub async fn primitives() {
    detach_metadata_parity_cases();
    // Fixed policy labels retain every native backend; platform availability is actual probe evidence.
    for name in [
        "apfs",
        "btrfs",
        "zfs",
        "reflink",
        "overlayfs",
        "projfs",
        "block-clone",
        "rcopy",
    ] {
        assert!(ara_iso::BackendKind::from_str(name).is_some(), "{name}");
    }
    let f = Fixture::new("primitives");
    let not_repo = f._temp.path().join("not-repo");
    fs::create_dir(&not_repo).unwrap();
    assert!(worktree::repo_root(&not_repo).is_err());
    assert!(ara_iso::BackendKind::from_str("not-a-backend").is_none());
    assert_eq!(
        worktree::repo_root(&f.root).unwrap(),
        GitRepo::require(&f.root).unwrap().root()
    );
    fs::create_dir_all(f.root.join(".jj/repo")).unwrap();
    assert!(worktree::repo_root(&f.root).is_ok());
    let pure = f.root.join("pure");
    fs::create_dir_all(pure.join(".jj/repo")).unwrap();
    assert!(
        worktree::repo_root(&pure)
            .unwrap_err()
            .to_string()
            .contains("Jujutsu")
    );
    let inner = pure.join("git");
    fs::create_dir(&inner).unwrap();
    init(&inner);
    git(
        &inner,
        &["commit", "-q", "--allow-empty", "-m", "inner baseline"],
    );
    assert_eq!(
        worktree::repo_root(&inner).unwrap(),
        GitRepo::require(&inner).unwrap().root()
    );
    // Source staged + unstaged remaining budget, and stat-before-untracked encoding.
    fs::write(f.root.join("oversized"), vec![42; 4096]).unwrap();
    assert!(worktree::capture_repo_baseline(&f.root, 512).is_err());
    fs::remove_file(f.root.join("oversized")).unwrap();
    fs::write(f.root.join("other.txt"), "stage\n".repeat(100)).unwrap();
    git(&f.root, &["add", "other.txt"]);
    assert!(worktree::capture_repo_baseline(&f.root, 16).is_err());
    let staged_len = GitRepo::require(&f.root)
        .unwrap()
        .diff_text(&DiffOptions {
            cached: true,
            binary: true,
            ..Default::default()
        })
        .unwrap()
        .len();
    fs::write(f.root.join("tracked.txt"), "unstaged\n".repeat(100)).unwrap();
    assert!(worktree::capture_repo_baseline(&f.root, staged_len + 16).is_err());
    // Real linked parent and detached copied metadata; child commits cannot move parent HEAD/index.
    let linked = f._temp.path().join("linked");
    git(
        &f.root,
        &[
            "worktree",
            "add",
            "-q",
            linked.to_str().unwrap(),
            "-b",
            "linked",
        ],
    );
    let mut request = f.request("linked", "patch");
    request.repo_root = linked.clone();
    let before_head = git(&linked, &["rev-parse", "HEAD"]);
    let before_index = fs::read(
        GitRepo::require(&linked)
            .unwrap()
            .info()
            .git_dir
            .join("index"),
    )
    .unwrap();
    let lease = IsolationLease::create(request).await.unwrap();
    assert!(lease.cwd().join(".git").is_dir());
    fs::write(lease.cwd().join("child-only"), "actual child").unwrap();
    git(lease.cwd(), &["add", "child-only"]);
    git(lease.cwd(), &["commit", "-qm", "child-private"]);
    assert_eq!(git(&linked, &["rev-parse", "HEAD"]), before_head);
    assert_eq!(
        fs::read(
            GitRepo::require(&linked)
                .unwrap()
                .info()
                .git_dir
                .join("index")
        )
        .unwrap(),
        before_index
    );
    assert_eq!(lease.finalize(Ok(())).await.unwrap().status, "released");
    // Physical paths are compact even for a long public task id.
    let long = f.lease(&"x".repeat(512), "patch").await;
    assert!(long.cwd().file_name().unwrap() == "m");
    assert!(
        long.cwd()
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .len()
            < 32
    );
    long.finalize(Ok(())).await.unwrap();
    #[cfg(unix)]
    {
        let symlink_fixture = Fixture::new("symlink-budget");
        let target = symlink_fixture._temp.path().join("large-target");
        fs::write(&target, vec![42; 8192]).unwrap();
        std::os::unix::fs::symlink(&target, symlink_fixture.root.join("link")).unwrap();
        assert!(
            worktree::capture_repo_baseline(&symlink_fixture.root, 512).is_ok(),
            "symlink target bytes are not buffered baseline file bytes"
        );
    }
}

pub async fn git_capture() {
    // Hold the actual canonical parent gate until cancellation reaches the original retained apply.
    let cancelled = Fixture::new("cancel-before-apply");
    fs::write(
        cancelled.root.join("other.txt"),
        "user staged before cancel\n",
    )
    .unwrap();
    git(&cancelled.root, &["add", "other.txt"]);
    fs::write(cancelled.root.join("user-untracked"), "preserved WIP").unwrap();
    let lease = cancelled.lease("cancel-before-apply", "patch").await;
    let permit = lease.admit().await.unwrap();
    fs::write(
        lease.cwd().join("agent-cancelled"),
        "captured actual agent edit",
    )
    .unwrap();
    let capture = lease.capture_run_end(&permit).await.unwrap();
    drop(permit);
    let before_head = git(&cancelled.root, &["rev-parse", "HEAD"]);
    let before_index = cancelled.staged();
    let before_worktree = fs::read(cancelled.root.join("other.txt")).unwrap();
    let parent_gate = worktree::repo_gate(&cancelled.root).unwrap();
    let held = parent_gate.lock_owned().await;
    let token = tokio_util::sync::CancellationToken::new();
    let operation = {
        let lease = lease.clone();
        let capture = capture.clone();
        let token = token.clone();
        tokio::spawn(async move { lease.apply_run_end_cancellable(&capture, token, None).await })
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while lease.snapshot()["apply_flights"] != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!operation.is_finished());
    token.cancel();
    drop(held);
    let result = operation.await.unwrap().unwrap();
    assert_eq!(result.status, "cancelled_before_mutation");
    assert!(!result.changes_applied);
    assert!(!result.had_any_changes);
    assert!(result.error.as_deref().unwrap().contains("cancelled"));
    assert!(result.durable_receipt.is_file());
    assert!(
        !capture
            .durable_receipt
            .with_file_name("apply-started.json")
            .exists()
    );
    assert_eq!(git(&cancelled.root, &["rev-parse", "HEAD"]), before_head);
    assert_eq!(cancelled.staged(), before_index);
    assert_eq!(
        fs::read(cancelled.root.join("other.txt")).unwrap(),
        before_worktree
    );
    assert!(!cancelled.root.join("agent-cancelled").exists());
    assert_eq!(
        fs::read_to_string(cancelled.root.join("user-untracked")).unwrap(),
        "preserved WIP"
    );
    // A later uncancelled observer joins the original known-not-applied receipt, never replays Git.
    assert_eq!(
        lease.apply_run_end(&capture).await.unwrap().status,
        "cancelled_before_mutation"
    );
    lease.finalize(Ok(())).await.unwrap();
    // Source #4136: nearby baseline WIP context must be subtracted even when the agent commits it.
    for committed in [false, true] {
        let f = Fixture::new("nearby-wip");
        let original = fs::read_to_string(f.root.join("tracked.txt")).unwrap();
        let dirty = original.replace("two\n", "parent WIP two\n");
        fs::write(f.root.join("tracked.txt"), &dirty).unwrap();
        let lease = f
            .lease(
                if committed {
                    "nearby-committed"
                } else {
                    "nearby-uncommitted"
                },
                "branch",
            )
            .await;
        let permit = lease.admit().await.unwrap();
        let changed = dirty.replace("five\n", "agent five\n");
        fs::write(lease.cwd().join("tracked.txt"), &changed).unwrap();
        if committed {
            git(lease.cwd(), &["add", "-A"]);
            git(lease.cwd(), &["commit", "-qm", "nearby original message"]);
        }
        let capture = lease.capture_run_end(&permit).await.unwrap();
        drop(permit);
        let branch = capture
            .branch
            .as_ref()
            .unwrap()
            .branch_name
            .as_ref()
            .unwrap();
        let branch_file = git(&f.root, &["show", &format!("{branch}:tracked.txt")]);
        assert!(branch_file.contains("agent five"));
        assert!(!branch_file.contains("parent WIP two"));
        let result = lease.apply_run_end(&capture).await.unwrap();
        assert!(result.changes_applied, "{result:?}");
        assert_eq!(
            fs::read_to_string(f.root.join("tracked.txt")).unwrap(),
            changed
        );
        lease.finalize(Ok(())).await.unwrap();
    }
    // An already landed first pick is skipped while the rest of the real commit range still lands.
    let f = Fixture::new("empty-pick");
    let lease = f.lease("empty-pick", "branch").await;
    let permit = lease.admit().await.unwrap();
    for file in ["already-picked", "remaining-pick"] {
        fs::write(lease.cwd().join(file), file).unwrap();
        git(lease.cwd(), &["add", file]);
        git(lease.cwd(), &["commit", "-qm", file]);
    }
    let capture = lease.capture_run_end(&permit).await.unwrap();
    drop(permit);
    let branch = capture.branch.as_ref().unwrap();
    let first = git(
        &f.root,
        &[
            "rev-list",
            "--reverse",
            &format!(
                "{}..{}",
                branch.base_sha,
                branch.branch_name.as_ref().unwrap()
            ),
        ],
    )
    .lines()
    .next()
    .unwrap()
    .to_owned();
    git(&f.root, &["cherry-pick", &first]);
    assert!(lease.apply_run_end(&capture).await.unwrap().changes_applied);
    assert!(f.root.join("remaining-pick").is_file());
    lease.finalize(Ok(())).await.unwrap();
    for mode in ["patch", "branch"] {
        let f = Fixture::new(mode);
        // Parent has unrelated staged, unstaged and untracked WIP, plus an unrelated older stash.
        fs::write(f.root.join("stash-only"), "stash").unwrap();
        git(&f.root, &["stash", "push", "-u", "-m", "unrelated stash"]);
        let stash = git(&f.root, &["rev-parse", "refs/stash"]);
        fs::write(f.root.join("other.txt"), "parent staged\n").unwrap();
        git(&f.root, &["add", "other.txt"]);
        fs::write(f.root.join("user-only"), "parent untracked\n").unwrap();
        let staged = f.staged();
        let head = git(&f.root, &["rev-parse", "HEAD"]);
        let lease = f.lease(&format!("capture-{mode}"), mode).await;
        let permit = lease.admit().await.unwrap();
        fs::write(lease.cwd().join("binary.bin"), [0, 9, 0, 8, 255]).unwrap();
        fs::write(lease.cwd().join("agent-only.txt"), "agent\n").unwrap();
        if mode == "branch" {
            git(lease.cwd(), &["add", "-A"]);
            git(lease.cwd(), &["commit", "-qm", "agent message one"]);
            fs::write(lease.cwd().join("trailing"), "trailing edit").unwrap();
        }
        let capture = lease.capture_run_end(&permit).await.unwrap();
        drop(permit);
        assert!(capture.changed);
        assert!(capture.patch_path.is_file());
        assert_eq!(git(&f.root, &["rev-parse", "HEAD"]), head);
        assert_eq!(f.staged(), staged);
        assert!(!f.root.join("agent-only.txt").exists());
        let patch = fs::read_to_string(&capture.patch_path).unwrap();
        assert!(patch.contains("GIT binary patch"));
        assert!(!patch.contains("parent staged"));
        assert!(!patch.contains("parent untracked"));
        let applied = lease.apply_run_end(&capture).await.unwrap();
        assert!(applied.changes_applied, "{:?}", applied);
        assert_eq!(
            fs::read(f.root.join("binary.bin")).unwrap(),
            [0, 9, 0, 8, 255]
        );
        assert_eq!(
            fs::read_to_string(f.root.join("user-only")).unwrap(),
            "parent untracked\n"
        );
        assert_eq!(git(&f.root, &["rev-parse", "refs/stash"]), stash);
        let staged_diff = git(&f.root, &["diff", "--cached"]);
        assert!(staged_diff.contains("parent staged"));
        if mode == "branch" {
            assert!(git(&f.root, &["log", "--format=%s", "-3"]).contains("agent message one"));
            assert!(f.root.join("trailing").is_file());
        }
        // Repeat integration joins the same receipt, and unchanged release cannot create a duplicate branch.
        assert_eq!(
            lease.apply_run_end(&capture).await.unwrap().capture_id,
            applied.capture_id
        );
        let released = lease.finalize(Ok(())).await.unwrap();
        assert_eq!(released.status, "released");
        assert!(capture.patch_path.is_file());
        assert_eq!(released.capture.unwrap().fingerprint, capture.fingerprint);
    }
    // Nested dirty stage/WIP restoration uses its own exact stash and binary-safe artifact.
    let f = Fixture::new("nested");
    let nested = f.root.join("nested");
    fs::create_dir(&nested).unwrap();
    init(&nested);
    fs::write(nested.join("n"), "base\n").unwrap();
    fs::write(nested.join("wip"), "base\n").unwrap();
    git(&nested, &["add", "."]);
    git(&nested, &["commit", "-qm", "nested baseline"]);
    fs::write(nested.join("wip"), "nested staged\n").unwrap();
    git(&nested, &["add", "wip"]);
    let lease = f.lease("nested", "patch").await;
    let permit = lease.admit().await.unwrap();
    fs::write(lease.cwd().join("nested/n"), "agent nested\n").unwrap();
    let capture = lease.capture_run_end(&permit).await.unwrap();
    drop(permit);
    assert_eq!(capture.nested_patch_paths.len(), 1);
    assert!(lease.apply_run_end(&capture).await.unwrap().changes_applied);
    assert_eq!(
        fs::read_to_string(nested.join("n")).unwrap(),
        "agent nested\n"
    );
    assert!(git(&nested, &["diff", "--cached"]).contains("nested staged"));
    lease.finalize(Ok(())).await.unwrap();
    assert!(capture.nested_patch_paths[0].patch_path.exists());
    // Own nested stash restoration conflict is reported, with the stash and committed agent work preserved.
    let f = Fixture::new("nested-stash-conflict");
    let nested = f.root.join("nested");
    fs::create_dir(&nested).unwrap();
    init(&nested);
    fs::write(nested.join("n"), "base\n").unwrap();
    git(&nested, &["add", "."]);
    git(&nested, &["commit", "-qm", "base"]);
    let lease = f.lease("nested-stash-conflict", "patch").await;
    let permit = lease.admit().await.unwrap();
    fs::write(lease.cwd().join("nested/n"), "agent conflicting value\n").unwrap();
    let capture = lease.capture_run_end(&permit).await.unwrap();
    drop(permit);
    fs::write(nested.join("n"), "user conflicting WIP\n").unwrap();
    git(&nested, &["add", "n"]);
    let applied = lease.apply_run_end(&capture).await.unwrap();
    assert!(applied.changes_applied);
    assert!(!applied.warnings.is_empty());
    assert!(!git(&nested, &["rev-parse", "refs/stash"]).is_empty());
    assert_eq!(git(&nested, &["show", "HEAD:n"]), "agent conflicting value");
    lease.finalize(Ok(())).await.unwrap();
    // Clean baseline preserves each original author/message and one trailing commit.
    let f = Fixture::new("history");
    let lease = f.lease("history", "branch").await;
    let permit = lease.admit().await.unwrap();
    for (file, msg) in [
        ("first", "first original message"),
        ("second", "second original message"),
    ] {
        fs::write(lease.cwd().join(file), msg).unwrap();
        git(lease.cwd(), &["add", file]);
        git(lease.cwd(), &["commit", "-qm", msg]);
    }
    fs::write(lease.cwd().join("last"), "trailing").unwrap();
    let receipt = lease.capture_run_end(&permit).await.unwrap();
    drop(permit);
    assert!(lease.apply_run_end(&receipt).await.unwrap().changes_applied);
    let log = git(&f.root, &["log", "--format=%s", "-3"]);
    assert!(
        log.contains("first original message")
            && log.contains("second original message")
            && log.contains("isolated agent changes")
    );
    lease.finalize(Ok(())).await.unwrap();
    // WIP new/untracked files modified by agent use the source WIP-seeded tree fallback.
    for staged in [false, true] {
        let f = Fixture::new("wip-overlap");
        fs::write(f.root.join("new-wip"), "user baseline\n").unwrap();
        if staged {
            git(&f.root, &["add", "new-wip"]);
        }
        let lease = f
            .lease(
                if staged {
                    "staged-new"
                } else {
                    "untracked-new"
                },
                "branch",
            )
            .await;
        let permit = lease.admit().await.unwrap();
        fs::write(
            lease.cwd().join("new-wip"),
            "user baseline\nagent addition\n",
        )
        .unwrap();
        let receipt = lease.capture_run_end(&permit).await.unwrap();
        drop(permit);
        let branch = receipt
            .branch
            .as_ref()
            .unwrap()
            .branch_name
            .as_ref()
            .unwrap();
        assert!(git(&f.root, &["show", &format!("{branch}:new-wip")]).contains("agent addition"));
        lease.finalize(Ok(())).await.unwrap();
    }
}

pub async fn ownership() {
    // The same short publication mutex orders warm publication before sealing, or rejects it after sealing.
    let ordering = Fixture::new("publication-seal");
    let ordered = ordering.lease("publication-seal", "patch").await;
    ordered.prepare_publish().unwrap().commit(&peer(1));
    let publish = ordered.prepare_republish(&peer(1)).unwrap();
    let sealing = {
        let lease = ordered.clone();
        tokio::task::spawn_blocking(move || lease.seal_admission())
    };
    publish.commit(&peer(1));
    sealing.await.unwrap().unwrap();
    ordered.seal_admission().unwrap();
    assert!(ordered.prepare_republish(&peer(1)).is_err());
    assert!(ordered.admit().await.is_err());
    assert_eq!(ordered.finalize(Ok(())).await.unwrap().status, "released");
    let f = Fixture::new("ownership");
    let lease = f.lease("same-task", "branch").await;
    lease.prepare_publish().unwrap().commit(&peer(1));
    lease.publish(&peer(1)).unwrap();
    assert!(lease.publish(&peer(2)).is_err());
    let permit = lease.admit().await.unwrap();
    fs::write(lease.cwd().join("first"), "run-end").unwrap();
    let first = lease.capture_run_end(&permit).await.unwrap();
    drop(permit);
    let branch = first.branch.as_ref().unwrap().branch_name.clone().unwrap();
    let tip = git(&f.root, &["rev-parse", &branch]);
    // Park is deliberately absent here: resource survives dropping a writer permit, without finalization.
    assert!(lease.cwd().is_dir());
    let warm = lease.admit().await.unwrap();
    assert_eq!(warm.resource_id(), lease.id());
    fs::write(lease.cwd().join("followup"), "warm followup").unwrap();
    drop(warm);
    let (a, b) = tokio::join!(lease.finalize(Ok(())), lease.finalize(Ok(())));
    let a = a.unwrap();
    let b = b.unwrap();
    assert_eq!(a.durable_receipt, b.durable_receipt);
    assert_eq!(a.status, "released");
    assert!(!lease.cwd().exists());
    assert_ne!(git(&f.root, &["rev-parse", &branch]), tip);
    assert!(git(&f.root, &["show", &format!("{branch}:followup")]).contains("warm followup"));
    assert!(
        !f.root.join("followup").exists(),
        "release must never apply follow-up parent changes"
    );
    assert!(lease.admit().await.is_err());
    // Final release waits active admission. A dropped observing task cannot cancel retained teardown.
    let lease = f.lease("lost-observer", "patch").await;
    let permit = lease.admit().await.unwrap();
    let cwd = lease.cwd().to_path_buf();
    fs::write(cwd.join("retained-worker-edit"), "actual content").unwrap();
    let observer = {
        let lease = lease.clone();
        tokio::spawn(async move { lease.finalize(Ok(())).await })
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while lease.snapshot()["sealed"] != true {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("original retained finalize flight seals admission before observer abort");
    assert!(cwd.exists());
    observer.abort();
    drop(permit);
    let release = lease.finalize(Ok(())).await.unwrap();
    assert_eq!(release.status, "released");
    assert!(!cwd.exists());
    assert!(release.capture.unwrap().patch_path.is_file());
    // Reusing a public Task id has a distinct physical leaf; old terminal release cannot remove it.
    let successor = f.lease("lost-observer", "patch").await;
    let successor_cwd = successor.cwd().to_path_buf();
    lease.finalize(Ok(())).await.unwrap();
    assert!(successor_cwd.exists());
    successor.finalize(Ok(())).await.unwrap();
}

pub async fn recovery() {
    #[cfg(windows)]
    {
        // Actual Windows filesystem sharing denial occurs at backend stop, after successful patch capture.
        use std::os::windows::fs::OpenOptionsExt;
        let stop_fault = Fixture::new("native-stop-denied");
        let lease = stop_fault.lease("native-stop-denied", "patch").await;
        fs::write(
            lease.cwd().join("locked"),
            "native stop must retain this copy",
        )
        .unwrap();
        let no_delete = fs::OpenOptions::new()
            .read(true)
            .share_mode(3)
            .open(lease.cwd().join("locked"))
            .unwrap();
        let release = lease.finalize(Ok(())).await.unwrap();
        assert_eq!(release.status, "retained");
        assert!(release.capture.as_ref().unwrap().patch_path.is_file());
        assert!(
            release
                .error
                .as_deref()
                .unwrap()
                .contains("backend stop failed")
        );
        assert!(lease.cwd().exists());
        drop(no_delete);
        assert_eq!(
            lease.finalize(Ok(())).await.unwrap().status,
            "retained",
            "failed native stop must not be replayed"
        );
    }
    let warm_fault = Fixture::new("warm-unknown");
    let warm_lease = warm_fault.lease("warm-unknown", "patch").await;
    warm_lease.publish(&peer(1)).unwrap();
    warm_lease
        .retain_unsettled_writer("warm actor's original close receipt is unknown")
        .unwrap();
    assert!(warm_lease.admit().await.is_err());
    assert!(warm_lease.prepare_republish(&peer(1)).is_err());
    let warm_release = warm_lease.finalize(Ok(())).await.unwrap();
    assert_eq!(warm_release.status, "retained");
    assert!(warm_lease.cwd().exists());
    // Actual filesystem artifact publication fault keeps the exact backend path in place.
    let f = Fixture::new("fault");
    let lease = f.lease("artifact-fault", "patch").await;
    let permit = lease.admit().await.unwrap();
    fs::write(lease.cwd().join("agent"), "recover me").unwrap();
    fs::write(f.artifacts.join(lease.id()), "blocks artifact directory").unwrap();
    assert!(lease.capture_run_end(&permit).await.is_err());
    drop(permit);
    // Remove only fixture-created obstruction so final receipt itself can become durable.
    fs::remove_file(f.artifacts.join(lease.id())).unwrap();
    let release = lease.finalize(Ok(())).await.unwrap();
    assert_eq!(release.status, "retained");
    assert!(lease.cwd().join("agent").exists());
    // Failed/unknown close cannot authorize any capture/stop/remove, even with valid source edits.
    let lease = f.lease("unknown-close", "patch").await;
    let cwd = lease.cwd().to_path_buf();
    fs::write(cwd.join("uncertain-writer"), "keep").unwrap();
    let release = lease
        .finalize(Err("original writer effects could not settle".into()))
        .await
        .unwrap();
    assert_eq!(release.status, "retained");
    assert!(release.capture.is_none());
    assert!(cwd.exists());
    // An external public-ref update is not overwritten or deleted by this owner; recovery artifacts remain.
    let f = Fixture::new("cas");
    let lease = f.lease("cas", "branch").await;
    let permit = lease.admit().await.unwrap();
    fs::write(lease.cwd().join("first"), "first").unwrap();
    let first = lease.capture_run_end(&permit).await.unwrap();
    drop(permit);
    let branch = first.branch.as_ref().unwrap().branch_name.clone().unwrap();
    git(
        &f.root,
        &["update-ref", &format!("refs/heads/{branch}"), "HEAD"],
    );
    let external = git(&f.root, &["rev-parse", &branch]);
    let permit = lease.admit().await.unwrap();
    fs::write(lease.cwd().join("second"), "second").unwrap();
    assert!(lease.capture_run_end(&permit).await.is_err());
    drop(permit);
    let release = lease.finalize(Ok(())).await.unwrap();
    assert_eq!(release.status, "retained");
    assert_eq!(git(&f.root, &["rev-parse", &branch]), external);
    assert!(lease.cwd().exists());
    assert!(first.patch_path.is_file());
    // Marker replacement refuses teardown. This exercises real ownership validation, not a counter callback.
    let lease = f.lease("marker", "patch").await;
    let leaf = lease.cwd().parent().unwrap();
    let marker = leaf.join("owner.json");
    let original = fs::read(&marker).unwrap();
    let mut json: serde_json::Value = serde_json::from_slice(&original).unwrap();
    json["resource_id"] = "another owner".into();
    fs::write(&marker, serde_json::to_vec(&json).unwrap()).unwrap();
    let release = lease.finalize(Ok(())).await.unwrap();
    assert_eq!(release.status, "retained");
    assert!(lease.cwd().exists());
    // Narrow branch-merge conflict leaves parent WIP untouched and branch recoverable.
    let f = Fixture::new("merge-conflict");
    let lease = f.lease("conflict", "branch").await;
    let permit = lease.admit().await.unwrap();
    fs::write(lease.cwd().join("other.txt"), "agent replacement\n").unwrap();
    let capture = lease.capture_run_end(&permit).await.unwrap();
    drop(permit);
    fs::write(f.root.join("other.txt"), "user conflicting WIP\n").unwrap();
    let before = f.staged();
    let result = lease.apply_run_end(&capture).await.unwrap();
    assert!(!result.changes_applied);
    assert_eq!(
        fs::read_to_string(f.root.join("other.txt")).unwrap(),
        "user conflicting WIP\n"
    );
    assert_eq!(f.staged(), before);
    assert!(
        GitRepo::require(&f.root)
            .unwrap()
            .ref_exists(
                capture
                    .branch
                    .as_ref()
                    .unwrap()
                    .branch_name
                    .as_ref()
                    .unwrap()
            )
            .unwrap()
    );
    lease.finalize(Ok(())).await.unwrap();
}
