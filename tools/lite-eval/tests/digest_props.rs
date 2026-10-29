//! Property 2 of the ara-lite-core-capability design for the digest rules in
//! `lite_eval::digest`.
//!
//! * `lite-eval-tree-v1`: random fixture trees (Unicode paths, binary content,
//!   files created in a random order, extra empty directories) are compared
//!   with an independent model of the documented concatenation, then changed
//!   once (bytes, relative path, an added or a removed file).
//! * `lite-eval-worktree-diff-v1`: every case copies one prepared repository,
//!   so a case needs no `git init` or `git commit`. It applies a random mix of
//!   staged, unstaged, staged-then-unstaged and untracked changes; then changes
//!   that only touch the rule's exclusions or `.gitignore`d paths, optionally
//!   staged; then one change the rule must see. The copied index carries stale
//!   stat data, so a diff that refreshed the index would be caught.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use lite_eval::{
    common::{sha256_file, sha256_hex, Credentials},
    digest::{tree_digest, worktree_diff, DiffRule, TreeEntry, WorktreeDiff, TREE_RULE, WORKTREE_DIFF_EXCLUSIONS},
};
use proptest::{
    collection::{btree_map, vec},
    prelude::*,
    sample::{select, Index},
    test_runner::{TestCaseError, TestRunner},
};

/// 256 cases per property (tasks.md 属性测试). Nothing is written into the
/// repository on failure; the report carries the minimised input instead.
fn config() -> ProptestConfig {
    ProptestConfig { failure_persistence: None, ..ProptestConfig::with_cases(256) }
}

fn write_file(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap()
}

// ---------------------------------------------------------------------------
// lite-eval-tree-v1

// No directory name equals a file name and no two names differ only in case
// (NTFS is case-insensitive). In UTF-8 byte order `B` sorts before `a` and
// U+FF61 before U+1F600; NTFS lists both pairs the other way round, so the
// model comparison also shows that the listing order does not matter.
static TREE_DIRS: [&str; 5] = ["d", "E", "sp ace", "中文", "\u{1f600}"];
static TREE_FILES: [&str; 7] = ["a.txt", "B.bin", "\u{ff61}.txt", "\u{1f600}.txt", "ä.c", "z", "Z0.h"];

type Files = Vec<(String, Vec<u8>)>;

#[derive(Clone, Debug)]
enum TreeChange {
    /// Flip one byte (size unchanged) or append one.
    Bytes {
        file: Index,
        at: Index,
        append: bool,
    },
    /// The same bytes under another relative path: renamed, or moved into a new directory.
    Rename {
        file: Index,
        move_dir: bool,
    },
    Add {
        name: &'static str,
        bytes: Vec<u8>,
    },
    Remove {
        file: Index,
    },
}

/// Files in generation order, the same files in a shuffled order, and one change.
fn tree_case() -> impl Strategy<Value = (Files, Files, TreeChange)> {
    let path = (vec(select(TREE_DIRS.to_vec()), 0..=2), select(TREE_FILES.to_vec()))
        .prop_map(|(dirs, file)| dirs.into_iter().chain([file]).collect::<Vec<_>>().join("/"));
    let change = prop_oneof![
        (any::<Index>(), any::<Index>(), any::<bool>()).prop_map(|(file, at, append)| TreeChange::Bytes {
            file,
            at,
            append
        }),
        (any::<Index>(), any::<bool>()).prop_map(|(file, move_dir)| TreeChange::Rename { file, move_dir }),
        (select(vec!["a.txt", "\u{1f600}.bin"]), vec(any::<u8>(), 0..16))
            .prop_map(|(name, bytes)| TreeChange::Add { name, bytes }),
        any::<Index>().prop_map(|file| TreeChange::Remove { file }),
    ];
    (btree_map(path, vec(any::<u8>(), 0..64), 1..8), change).prop_flat_map(|(files, change)| {
        let files: Files = files.into_iter().collect();
        (Just(files.clone()), Just(files).prop_shuffle(), Just(change))
    })
}

/// Independent statement of `lite-eval-tree-v1`; `String` keys iterate in
/// UTF-8 byte order.
fn model_entries(files: &BTreeMap<String, Vec<u8>>) -> Vec<TreeEntry> {
    files
        .iter()
        .map(|(path, bytes)| TreeEntry { path: path.clone(), size: bytes.len() as u64, sha256: sha256_hex(bytes) })
        .collect()
}

fn model_sha(files: &BTreeMap<String, Vec<u8>>) -> String {
    let concatenation: String = model_entries(files)
        .iter()
        .map(|entry| format!("{}\0{}\0{}\n", entry.path, entry.size, entry.sha256))
        .collect();
    sha256_hex(concatenation.as_bytes())
}

/// Apply `change` on disk under `root` and to the model `files`.
fn apply_tree_change(files: &mut BTreeMap<String, Vec<u8>>, root: &Path, change: &TreeChange) {
    let paths: Vec<String> = files.keys().cloned().collect();
    match change {
        TreeChange::Bytes { file, at, append } => {
            let path = file.get(&paths);
            let bytes = files.get_mut(path).unwrap();
            if *append || bytes.is_empty() {
                bytes.push(0x5a);
            } else {
                let at = at.index(bytes.len());
                bytes[at] ^= 0x01;
            }
            write_file(&root.join(path), bytes);
        }
        TreeChange::Rename { file, move_dir } => {
            let from = file.get(&paths);
            let to = match (move_dir, from.rsplit_once('/')) {
                (true, _) => format!("moved/{from}"),
                (false, Some((dir, name))) => format!("{dir}/r_{name}"),
                (false, None) => format!("r_{from}"),
            };
            fs::create_dir_all(root.join(&to).parent().unwrap()).unwrap();
            fs::rename(root.join(from), root.join(&to)).unwrap();
            let bytes = files.remove(from).unwrap();
            files.insert(to, bytes);
        }
        TreeChange::Add { name, bytes } => {
            let path = format!("added/{name}");
            write_file(&root.join(&path), bytes);
            files.insert(path, bytes.clone());
        }
        TreeChange::Remove { file } => {
            let path = file.get(&paths);
            fs::remove_file(root.join(path)).unwrap();
            files.remove(path);
        }
    }
}

proptest! {
    #![proptest_config(config())]

    // Feature: ara-lite-core-capability, Property 2: 摘要规则确定且对改动敏感
    // (lite-eval-tree-v1 part; the lite-eval-worktree-diff-v1 part follows below.)
    #[test]
    fn tree_digest_is_deterministic_order_independent_and_change_sensitive(
        (files, shuffled, change) in tree_case()
    ) {
        let model: BTreeMap<String, Vec<u8>> = files.iter().cloned().collect();
        let dir = tempfile::tempdir().unwrap();
        let (first, second) = (dir.path().join("first"), dir.path().join("second"));
        for (path, bytes) in &files {
            write_file(&first.join(path), bytes);
        }
        for (path, bytes) in &shuffled {
            write_file(&second.join(path), bytes);
        }
        fs::create_dir_all(second.join("empty/nested")).unwrap();

        let digest = tree_digest(&first).unwrap();
        prop_assert_eq!(digest.rule, TREE_RULE);
        prop_assert_eq!(&digest.entries, &model_entries(&model));
        prop_assert_eq!(&digest.sha256, &model_sha(&model));
        prop_assert_eq!(&tree_digest(&first).unwrap(), &digest, "second computation of the same tree");
        prop_assert_eq!(&tree_digest(&second).unwrap(), &digest, "other creation order plus empty directories");

        // A single-file fixture is the one-entry tree named after the file.
        let (path, bytes) = &files[0];
        let single = BTreeMap::from([(basename(path).to_owned(), bytes.clone())]);
        prop_assert_eq!(tree_digest(&first.join(path)).unwrap().sha256, model_sha(&single));

        let mut changed = model;
        apply_tree_change(&mut changed, &second, &change);
        let after = tree_digest(&second).unwrap();
        prop_assert_ne!(&after.sha256, &digest.sha256, "{:?} left the digest unchanged", change);
        prop_assert_eq!(after.sha256, model_sha(&changed));
    }
}

// ---------------------------------------------------------------------------
// lite-eval-worktree-diff-v1

const EVIDENCE: &str = "crates/ara-agent-lite-core/docs/evidence/lite-capability";
const CODEBASE_MEMORY: &str = ".codebase-memory";

/// Committed files outside the exclusions, with their committed bytes.
const POOL: [(&str, &[u8]); 5] = [
    ("src/lib.rs", b"pub fn f() {}\n"),
    ("notes.txt", b"line 1\r\nline 2\r\n"),
    ("data/blob.bin", b"\0\x01\x02binary\xff\0"),
    ("empty.txt", b""),
    ("docs/readme.md", b"# readme\n"),
];
const GITIGNORE: &[u8] = b"/ignored*\n*.log\n";
const EVIDENCE_TRACKED: &str = "crates/ara-agent-lite-core/docs/evidence/lite-capability/progress-ledger.jsonl";
const MEMORY_TRACKED: &str = ".codebase-memory/artifact.json";

/// Paths the rule must never see: under an exclusion (committed or new) or
/// matched by `.gitignore`.
static HIDDEN_PATHS: [&str; 10] = [
    EVIDENCE_TRACKED,
    "crates/ara-agent-lite-core/docs/evidence/lite-capability/new.json",
    "crates/ara-agent-lite-core/docs/evidence/lite-capability/gates/g.json",
    MEMORY_TRACKED,
    ".codebase-memory/graph.db.zst",
    ".codebase-memory/nested/x.bin",
    "ignored.out",
    "ignored-dir/x.txt",
    "run.log",
    "src/build.log",
];

fn is_pool(path: &str) -> bool {
    POOL.iter().any(|(pool_path, _)| *pool_path == path)
}

/// Header suffix git writes for `path` in both diff parts
/// (`diff --git a/<path> b/<path>`); every path used here is plain ASCII, so
/// git does not quote it.
fn header(path: &str) -> Vec<u8> {
    format!(" b/{path}\n").into_bytes()
}

/// Whether `part` has a diff for `path`. `git diff` detects renames by
/// default (`diff.renames`), so a deleted file may show up only as the source
/// of a rename to a staged new file.
fn mentioned(part: &[u8], path: &str) -> bool {
    find(part, &header(path)).is_some() || find(part, format!("\nrename from {path}\n").as_bytes()).is_some()
}

fn run_git(dir: &Path, args: &[&str]) -> Vec<u8> {
    let output = Command::new("git").current_dir(dir).args(args).stdin(Stdio::null()).output().unwrap();
    assert!(output.status.success(), "git {args:?} in {}: {}", dir.display(), String::from_utf8_lossy(&output.stderr));
    output.stdout
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// One committed repository that every case copies. Its configuration is
/// repository-local; the global git configuration is never touched.
struct Template {
    _dir: tempfile::TempDir,
    root: PathBuf,
    head: String,
}

fn prepare_template() -> Template {
    let dir = tempfile::tempdir().unwrap();
    let (root, no_hooks) = (dir.path().join("repo"), dir.path().join("empty-git-template"));
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&no_hooks).unwrap();
    let template = format!("--template={}", no_hooks.display());
    run_git(&root, &["-c", "init.defaultBranch=main", "init", "-q", &template]);
    for (key, value) in [
        ("core.autocrlf", "false"),
        ("user.name", "lite-eval test"),
        ("user.email", "lite-eval@example.invalid"),
        ("commit.gpgsign", "false"),
    ] {
        run_git(&root, &["config", key, value]);
    }
    for (path, bytes) in POOL {
        write_file(&root.join(path), bytes);
    }
    write_file(&root.join(".gitignore"), GITIGNORE);
    write_file(&root.join(EVIDENCE_TRACKED), b"{\"seq\":1}\n");
    write_file(&root.join(MEMORY_TRACKED), b"{\"cache\":1}\n");
    run_git(&root, &["add", "--all"]);
    run_git(&root, &["commit", "-q", "-m", "base"]);
    let head = String::from_utf8(run_git(&root, &["rev-parse", "HEAD"])).unwrap().trim().to_owned();
    Template { _dir: dir, root, head }
}

#[derive(Clone, Debug)]
enum PoolChange {
    Unchanged,
    Unstaged(Vec<u8>),
    Staged(Vec<u8>),
    StagedThenUnstaged(Vec<u8>, Vec<u8>),
    /// Staged, then the working tree is restored to the committed bytes.
    StagedThenReverted(Vec<u8>),
    Deleted,
    DeletedStaged,
}

#[derive(Clone, Debug)]
enum NewFile {
    Untracked(Vec<u8>),
    Staged(Vec<u8>),
    StagedThenUnstaged(Vec<u8>, Vec<u8>),
}

#[derive(Clone, Debug)]
enum Hidden {
    Write(&'static str, Vec<u8>),
    Delete(&'static str),
}

/// One change the rule must see. Each variant falls back to an always-visible
/// change when its target does not exist in the case.
#[derive(Clone, Debug)]
enum Visible {
    /// New bytes for a committed file (forced to differ from its current bytes).
    WritePool(Index, Vec<u8>),
    DeletePool(Index),
    /// Move a committed file to `moved/<name>`.
    MovePool(Index),
    AddUntracked(&'static str, Vec<u8>),
    /// New bytes for a file the case added (forced to differ from its current bytes).
    WriteAdded(Index, Vec<u8>),
    /// Move an untracked file to `renamed/<name>`.
    MoveUntracked(Index),
}

#[derive(Clone, Debug)]
struct Scenario {
    pool: [PoolChange; 5],
    added: BTreeMap<String, NewFile>,
    hidden: Vec<Hidden>,
    stage_hidden: bool,
    visible: Visible,
}

fn content() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![vec(any::<u8>(), 0..48), vec(select(b"ab \t\r\n".to_vec()), 0..48)]
}

fn pool_change() -> impl Strategy<Value = PoolChange> {
    prop_oneof![
        3 => Just(PoolChange::Unchanged),
        2 => content().prop_map(PoolChange::Unstaged),
        2 => content().prop_map(PoolChange::Staged),
        1 => (content(), content()).prop_map(|(staged, unstaged)| PoolChange::StagedThenUnstaged(staged, unstaged)),
        1 => content().prop_map(PoolChange::StagedThenReverted),
        1 => Just(PoolChange::Deleted),
        1 => Just(PoolChange::DeletedStaged),
    ]
}

fn new_file() -> impl Strategy<Value = NewFile> {
    prop_oneof![
        2 => content().prop_map(NewFile::Untracked),
        1 => content().prop_map(NewFile::Staged),
        1 => (content(), content()).prop_map(|(staged, unstaged)| NewFile::StagedThenUnstaged(staged, unstaged)),
    ]
}

/// One in five cases leaves the visible part of the tree clean.
fn changes() -> impl Strategy<Value = ([PoolChange; 5], BTreeMap<String, NewFile>)> {
    let added_path = (
        select(vec!["", "src/", "new/", "new/deep/"]),
        select(vec!["n1", "n2", "x", "y"]),
        select(vec![".txt", ".bin", ""]),
    )
        .prop_map(|(dir, name, extension)| format!("{dir}{name}{extension}"));
    prop_oneof![
        1 => Just((std::array::from_fn(|_| PoolChange::Unchanged), BTreeMap::new())),
        4 => (proptest::array::uniform::<_, 5>(pool_change()), btree_map(added_path, new_file(), 0..4)),
    ]
}

fn scenario() -> impl Strategy<Value = Scenario> {
    let hidden = prop_oneof![
        3 => (select(HIDDEN_PATHS.to_vec()), content()).prop_map(|(path, bytes)| Hidden::Write(path, bytes)),
        1 => select(vec![EVIDENCE_TRACKED, MEMORY_TRACKED]).prop_map(Hidden::Delete),
    ];
    let visible = prop_oneof![
        (any::<Index>(), content()).prop_map(|(file, bytes)| Visible::WritePool(file, bytes)),
        any::<Index>().prop_map(Visible::DeletePool),
        any::<Index>().prop_map(Visible::MovePool),
        (select(vec!["added/a.txt", "added/z.bin", "0"]), content())
            .prop_map(|(path, bytes)| Visible::AddUntracked(path, bytes)),
        (any::<Index>(), content()).prop_map(|(file, bytes)| Visible::WriteAdded(file, bytes)),
        any::<Index>().prop_map(Visible::MoveUntracked),
    ];
    (changes(), vec(hidden, 1..4), any::<bool>(), visible).prop_map(|((pool, added), hidden, stage_hidden, visible)| {
        Scenario { pool, added, hidden, stage_hidden, visible }
    })
}

/// A case's copy of the template and the model of its visible state.
struct Worktree {
    root: PathBuf,
    /// Current bytes of every visible path the case touched or committed; `None` once absent.
    files: BTreeMap<String, Option<Vec<u8>>>,
    /// New paths that were added to the index.
    staged_new: BTreeSet<String>,
}

/// What the diff must contain, derived from the model only.
struct Expected {
    /// Paths with a tracked diff (part 1): committed files whose bytes changed or
    /// that are gone, and staged new files.
    tracked: BTreeSet<String>,
    /// Untracked visible files in path-byte order (part 2).
    untracked: Vec<String>,
    /// Committed files whose current bytes equal the commit.
    unchanged: Vec<&'static str>,
}

impl Worktree {
    fn new(root: PathBuf) -> Self {
        let files = POOL.iter().map(|(path, bytes)| (path.to_string(), Some(bytes.to_vec()))).collect();
        Self { root, files, staged_new: BTreeSet::new() }
    }

    fn current(&self, path: &str) -> Option<&[u8]> {
        self.files.get(path).and_then(|bytes| bytes.as_deref())
    }

    fn write(&mut self, path: &str, bytes: &[u8]) {
        write_file(&self.root.join(path), bytes);
        self.files.insert(path.to_owned(), Some(bytes.to_vec()));
    }

    /// Write bytes that differ from the current ones.
    fn rewrite(&mut self, path: &str, bytes: &[u8]) {
        let mut bytes = bytes.to_vec();
        if self.current(path) == Some(bytes.as_slice()) {
            bytes.push(b'!');
        }
        self.write(path, &bytes);
    }

    fn delete(&mut self, path: &str) {
        fs::remove_file(self.root.join(path)).unwrap();
        self.files.insert(path.to_owned(), None);
    }

    fn rename(&mut self, from: &str, to: &str) {
        fs::create_dir_all(self.root.join(to).parent().unwrap()).unwrap();
        fs::rename(self.root.join(from), self.root.join(to)).unwrap();
        let bytes = self.files.insert(from.to_owned(), None).flatten();
        self.files.insert(to.to_owned(), bytes);
    }

    fn stage(&self, paths: &[String]) {
        if !paths.is_empty() {
            let mut args = vec!["add", "--all", "--"];
            args.extend(paths.iter().map(String::as_str));
            run_git(&self.root, &args);
        }
    }

    fn expected(&self) -> Expected {
        let mut expected = Expected { tracked: BTreeSet::new(), untracked: Vec::new(), unchanged: Vec::new() };
        for (path, committed) in POOL {
            if self.current(path) == Some(committed) {
                expected.unchanged.push(path);
            } else {
                expected.tracked.insert(path.to_owned());
            }
        }
        for (path, bytes) in &self.files {
            if is_pool(path) || bytes.is_none() {
                continue;
            }
            if self.staged_new.contains(path) {
                expected.tracked.insert(path.clone());
            } else {
                expected.untracked.push(path.clone());
            }
        }
        expected
    }

    /// The case's visible files outside the commit; untracked ones only when `untracked_only`.
    fn added(&self, untracked_only: bool) -> Vec<String> {
        let keep = |path: &String| !is_pool(path) && !(untracked_only && self.staged_new.contains(path));
        self.files.iter().filter(|(path, bytes)| bytes.is_some() && keep(path)).map(|(path, _)| path.clone()).collect()
    }
}

/// Apply the staged, unstaged and untracked changes of `scenario` (one `git add` at most).
fn apply_changes(tree: &mut Worktree, scenario: &Scenario) {
    let (mut stage, mut afterwards) = (Vec::new(), Vec::new());
    for ((path, committed), change) in POOL.into_iter().zip(&scenario.pool) {
        match change {
            PoolChange::Unchanged => {}
            PoolChange::Unstaged(bytes) => tree.write(path, bytes),
            PoolChange::Staged(bytes) => {
                tree.write(path, bytes);
                stage.push(path.to_owned());
            }
            PoolChange::StagedThenUnstaged(staged, unstaged) => {
                tree.write(path, staged);
                stage.push(path.to_owned());
                afterwards.push((path.to_owned(), unstaged.clone()));
            }
            PoolChange::StagedThenReverted(staged) => {
                tree.write(path, staged);
                stage.push(path.to_owned());
                afterwards.push((path.to_owned(), committed.to_vec()));
            }
            PoolChange::Deleted => tree.delete(path),
            PoolChange::DeletedStaged => {
                tree.delete(path);
                stage.push(path.to_owned());
            }
        }
    }
    for (path, file) in &scenario.added {
        match file {
            NewFile::Untracked(bytes) => tree.write(path, bytes),
            NewFile::Staged(bytes) | NewFile::StagedThenUnstaged(bytes, _) => {
                tree.write(path, bytes);
                stage.push(path.clone());
                tree.staged_new.insert(path.clone());
            }
        }
        if let NewFile::StagedThenUnstaged(_, unstaged) = file {
            afterwards.push((path.clone(), unstaged.clone()));
        }
    }
    tree.stage(&stage);
    for (path, bytes) in afterwards {
        tree.write(&path, &bytes);
    }
}

/// Changes the rule must not see; the model is left as it is.
fn apply_hidden(tree: &Worktree, scenario: &Scenario) {
    for change in &scenario.hidden {
        match change {
            Hidden::Write(path, bytes) => write_file(&tree.root.join(path), bytes),
            Hidden::Delete(path) => match fs::remove_file(tree.root.join(path)) {
                Err(error) if error.kind() != ErrorKind::NotFound => panic!("cannot delete {path}: {error}"),
                _ => {}
            },
        }
    }
    if scenario.stage_hidden {
        // Both pathspecs always match index entries, even after a deletion.
        tree.stage(&[EVIDENCE.to_owned(), CODEBASE_MEMORY.to_owned()]);
    }
}

fn apply_visible(tree: &mut Worktree, visible: &Visible) {
    const FALLBACK: &str = "added/fallback.txt";
    match visible {
        Visible::WritePool(file, bytes) => tree.rewrite(file.get(&POOL).0, bytes),
        Visible::DeletePool(file) | Visible::MovePool(file) => {
            let path = file.get(&POOL).0;
            match (tree.current(path).is_some(), visible) {
                (true, Visible::DeletePool(_)) => tree.delete(path),
                (true, _) => tree.rename(path, &format!("moved/{}", basename(path))),
                (false, _) => tree.write(path, b"recreated\n"),
            }
        }
        Visible::AddUntracked(path, bytes) => tree.write(path, bytes),
        Visible::WriteAdded(file, bytes) => match tree.added(false) {
            added if added.is_empty() => tree.write(FALLBACK, bytes),
            added => tree.rewrite(file.get(&added), bytes),
        },
        Visible::MoveUntracked(file) => match tree.added(true) {
            untracked if untracked.is_empty() => tree.write(FALLBACK, b"fallback\n"),
            untracked => {
                let from = file.get(&untracked);
                tree.rename(from, &format!("renamed/{}", basename(from)));
            }
        },
    }
}

/// `worktree_diff`, failing the case when it changes a byte of `.git/index`.
fn diff_keeping_index(root: &Path) -> Result<WorktreeDiff, TestCaseError> {
    let index = fs::read(root.join(".git/index")).unwrap();
    let diff = worktree_diff(root).map_err(|error| TestCaseError::fail(format!("worktree_diff failed: {error:#}")))?;
    prop_assert!(fs::read(root.join(".git/index")).unwrap() == index, "worktree_diff modified .git/index");
    Ok(diff)
}

#[test]
fn copied_clean_index_and_excluded_empty_write_are_read_only() {
    let template = prepare_template();
    let case = tempfile::tempdir().unwrap();
    let root = case.path().join("repo");
    copy_tree(&template.root, &root);
    // A copied empty tracked file is sufficient: index stat data belongs to
    // the original directory. No hidden change has happened at this point.
    let first = diff_keeping_index(&root).unwrap();
    assert!(!first.dirty);
    write_file(&root.join(EVIDENCE_TRACKED), b"");
    let second = diff_keeping_index(&root).unwrap();
    assert!(!second.dirty);
    assert_eq!(second.bytes, first.bytes);
    write_file(&root.join("src/lib.rs"), b"pub fn f() { println!(\"visible\"); }\n");
    let third = diff_keeping_index(&root).unwrap();
    assert!(third.dirty);
    let control = Command::new("git")
        .current_dir(&root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(["diff", "--binary", "--no-textconv", "--no-ext-diff", "HEAD", "--", "."])
        .args(WORKTREE_DIFF_EXCLUSIONS.map(|path| format!(":(exclude){path}")))
        .output()
        .unwrap();
    assert!(control.status.success());
    assert_eq!(third.bytes, control.stdout, "disabling auto-refresh must preserve diff bytes");
}

fn check_worktree_case(template: &Template, scenario: &Scenario) -> Result<(), TestCaseError> {
    let case = tempfile::tempdir().unwrap();
    let mut tree = Worktree::new(case.path().join("repo"));
    copy_tree(&template.root, &tree.root);

    // 1. Staged, unstaged and untracked changes: the diff matches the model.
    apply_changes(&mut tree, scenario);
    let expected = tree.expected();
    let dirty = !expected.tracked.is_empty() || !expected.untracked.is_empty();
    let first = diff_keeping_index(&tree.root)?;
    prop_assert_eq!(&first.head, &template.head);
    prop_assert_eq!(&first.rule, &DiffRule::current());
    prop_assert_eq!(first.dirty, dirty);
    prop_assert_eq!(first.bytes.is_empty(), !dirty);
    prop_assert_eq!(&first.sha256, &dirty.then(|| sha256_hex(&first.bytes)));
    prop_assert_eq!(&first.untracked_files, &expected.untracked);
    prop_assert_eq!(first.tracked_bytes == 0, expected.tracked.is_empty());
    let (tracked_part, untracked_part) = first.bytes.split_at(first.tracked_bytes);
    for path in &expected.tracked {
        prop_assert!(mentioned(tracked_part, path), "no tracked diff for {}", path);
    }
    for path in &expected.unchanged {
        prop_assert!(!mentioned(&first.bytes, path), "diff for unchanged {}", path);
    }
    let mut from = 0;
    for path in &expected.untracked {
        prop_assert!(!mentioned(tracked_part, path), "untracked {} in the tracked part", path);
        let header = header(path);
        let at = find(&untracked_part[from..], &header);
        prop_assert!(at.is_some(), "untracked {} missing from part 2 or out of path-byte order", path);
        from += at.unwrap() + header.len();
    }

    // The saved diff is the complete diff, and its SHA-256 is the recorded one.
    let saved = case.path().join("worktree.diff");
    first.save(&saved, &Credentials::default()).unwrap();
    prop_assert!(fs::read(&saved).unwrap() == first.bytes, "saved diff differs from the computed bytes");
    if let Some(sha256) = &first.sha256 {
        prop_assert_eq!(&sha256_file(&saved).unwrap(), sha256);
    }

    // 2. Changes under the exclusions or matched by .gitignore, possibly staged:
    //    the same bytes as before (this second computation also checks determinism).
    apply_hidden(&tree, scenario);
    let second = diff_keeping_index(&tree.root)?;
    prop_assert!(second.bytes == first.bytes, "excluded or ignored changes {:?} altered the diff", scenario.hidden);
    prop_assert_eq!(&second.sha256, &first.sha256);
    prop_assert_eq!(second.dirty, first.dirty);
    prop_assert_eq!(second.tracked_bytes, first.tracked_bytes);
    prop_assert_eq!(&second.untracked_files, &first.untracked_files);

    // 3. One visible change: a different SHA-256.
    apply_visible(&mut tree, &scenario.visible);
    let third = diff_keeping_index(&tree.root)?;
    prop_assert!(third.dirty);
    prop_assert_eq!(&third.sha256, &Some(sha256_hex(&third.bytes)));
    prop_assert_ne!(&third.sha256, &first.sha256, "{:?} left the diff unchanged", scenario.visible);
    Ok(())
}

// Feature: ara-lite-core-capability, Property 2: 摘要规则确定且对改动敏感
// (lite-eval-worktree-diff-v1 part.) A manual runner lets the 256 cases share
// one prepared repository and removes it afterwards.
#[test]
fn worktree_diff_is_deterministic_honours_exclusions_and_is_change_sensitive() {
    assert_eq!(WORKTREE_DIFF_EXCLUSIONS, [EVIDENCE, CODEBASE_MEMORY], "test constants follow the rule");
    let template = prepare_template();
    let mut runner = TestRunner::new(config());
    if let Err(error) = runner.run(&scenario(), |case| check_worktree_case(&template, &case)) {
        panic!("{error}");
    }
}
