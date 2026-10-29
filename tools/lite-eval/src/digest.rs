//! Digest rules (design “fixture 摘要规则”), used both when a value is
//! registered and when it is checked again.
//!
//! * [`TREE_RULE`] fingerprints a fixture. Every regular file, sorted by its
//!   `/`-separated relative path in UTF-8 byte order, contributes
//!   `relpath \0 size \0 sha256(file) \n` (size in decimal ASCII, SHA-256 as
//!   lowercase hex) to one SHA-256. Symbolic links, junctions and other
//!   reparse points are refused, never followed; empty directories contribute
//!   nothing. A single-file fixture is a one-entry tree whose relative path is
//!   the file name, so a file hashes the same whether it is given alone or as
//!   the only file of a directory.
//! * [`WORKTREE_DIFF_RULE`] fingerprints the code version: the raw bytes of
//!   the tracked diff against HEAD, followed by one `/dev/null` diff per
//!   untracked file. [`DiffRule::current`] describes the exact commands for
//!   Run_Manifest and gate records.

use std::{
    fs::{self, Metadata},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{bail, ensure, Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::common::{sha256_file, sha256_hex, write_new, Credentials};

/// Rule id recorded next to every fixture SHA-256.
pub const TREE_RULE: &str = "lite-eval-tree-v1";

/// One regular file of a [`TreeDigest`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TreeEntry {
    /// Relative path with `/` separators.
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TreeDigest {
    pub rule: &'static str,
    pub sha256: String,
    /// Sorted by `path` bytes, the order that was hashed.
    pub entries: Vec<TreeEntry>,
}

/// [`TREE_RULE`] digest of a fixture directory or single fixture file.
pub fn tree_digest(root: &Path) -> Result<TreeDigest> {
    let metadata = fs::symlink_metadata(root).with_context(|| format!("cannot read fixture {}", root.display()))?;
    refuse_link(root, &metadata)?;
    let mut entries = Vec::new();
    if metadata.is_dir() {
        collect(root, "", &mut entries)?;
    } else if metadata.is_file() {
        let name = root.file_name().and_then(|name| name.to_str());
        let name = name.with_context(|| format!("fixture file name {} is not valid UTF-8", root.display()))?;
        entries.push(TreeEntry { path: name.to_owned(), size: metadata.len(), sha256: sha256_file(root)? });
    } else {
        bail!("fixture {} is neither a regular file nor a directory", root.display());
    }
    // `String` ordering is UTF-8 byte ordering.
    entries.sort_unstable_by(|left, right| left.path.cmp(&right.path));
    let mut digest = Sha256::new();
    for entry in &entries {
        digest.update(format!("{}\0{}\0{}\n", entry.path, entry.size, entry.sha256));
    }
    Ok(TreeDigest { rule: TREE_RULE, sha256: format!("{:x}", digest.finalize()), entries })
}

fn collect(dir: &Path, prefix: &str, entries: &mut Vec<TreeEntry>) -> Result<()> {
    for item in fs::read_dir(dir).with_context(|| format!("cannot list fixture directory {}", dir.display()))? {
        let path = item.with_context(|| format!("cannot list fixture directory {}", dir.display()))?.path();
        let name = path.file_name().and_then(|name| name.to_str());
        let name = name.with_context(|| format!("fixture path {} is not valid UTF-8", path.display()))?;
        let relpath = if prefix.is_empty() { name.to_owned() } else { format!("{prefix}/{name}") };
        let metadata = fs::symlink_metadata(&path).with_context(|| format!("cannot read {}", path.display()))?;
        refuse_link(&path, &metadata)?;
        if metadata.is_dir() {
            collect(&path, &relpath, entries)?;
        } else if metadata.is_file() {
            entries.push(TreeEntry { path: relpath, size: metadata.len(), sha256: sha256_file(&path)? });
        } else {
            bail!("fixture entry {} is neither a regular file nor a directory", path.display());
        }
    }
    Ok(())
}

pub(crate) fn refuse_link(path: &Path, metadata: &Metadata) -> Result<()> {
    #[cfg(windows)]
    let reparse_point = {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT != 0
    };
    #[cfg(not(windows))]
    let reparse_point = false;
    ensure!(
        !metadata.file_type().is_symlink() && !reparse_point,
        "{} is a symbolic link, junction or other reparse point; {TREE_RULE} refuses links instead of following them",
        path.display()
    );
    Ok(())
}

/// Rule id recorded next to every code-version diff SHA-256.
pub const WORKTREE_DIFF_RULE: &str = "lite-eval-worktree-diff-v1";

/// Repository-relative paths left out of both the tracked diff and the
/// untracked file list: the in-repo evidence index (appending ledger, gate
/// and review records must not change the code version) and the
/// `.codebase-memory` tool cache (rewritten by that tool without any source
/// change).
pub const WORKTREE_DIFF_EXCLUSIONS: [&str; 2] =
    ["crates/ara-agent-lite-core/docs/evidence/lite-capability", ".codebase-memory"];

/// Flags of every `git diff` call: binary patches, and the raw bytes even
/// when git configuration (for example `diff.astextplain.textconv`) or an
/// external diff driver would rewrite them.
pub const WORKTREE_DIFF_FLAGS: [&str; 3] = ["--binary", "--no-textconv", "--no-ext-diff"];

/// Disable optional Git locks. Porcelain diff's independent index refresh is
/// disabled explicitly in `tracked_diff_args` as well.
const GIT_ENVIRONMENT: (&str, &str) = ("GIT_OPTIONAL_LOCKS", "0");

/// Exact statement of [`WORKTREE_DIFF_RULE`] for Run_Manifest and gate records.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DiffRule {
    pub id: &'static str,
    pub exclusions: [&'static str; 2],
    pub diff_flags: [&'static str; 3],
    pub environment: String,
    /// Part 1, run from the repository root.
    pub tracked_diff: String,
    /// Lists part 2's files; NUL-separated, sorted by path bytes ascending.
    pub untracked_list: String,
    /// Part 2, per listed file; exit code 1 (files differ) is normal.
    pub untracked_diff: String,
    pub description: String,
}

impl DiffRule {
    pub fn current() -> Self {
        let line = |args: Vec<String>| format!("git {}", args.join(" "));
        let (tracked_diff, untracked_list) = (line(tracked_diff_args()), line(untracked_list_args()));
        let untracked_diff = line(untracked_diff_args("<path>"));
        let environment = format!("{}={}", GIT_ENVIRONMENT.0, GIT_ENVIRONMENT.1);
        let description = format!(
            "{WORKTREE_DIFF_RULE}: raw stdout of `{tracked_diff}`, followed for every path listed by \
             `{untracked_list}` in ascending path-byte order by the raw stdout of `{untracked_diff}` \
             (exit code 1 is normal); excluded from both parts: {}; git diff flags: {}; run from the \
             repository root with {environment} and diff.autoRefreshIndex=false; the index is never modified; an empty result is \
             recorded as dirty=false with no SHA-256",
            WORKTREE_DIFF_EXCLUSIONS.map(|path| format!("{path}/")).join(", "),
            WORKTREE_DIFF_FLAGS.join(" "),
        );
        Self {
            id: WORKTREE_DIFF_RULE,
            exclusions: WORKTREE_DIFF_EXCLUSIONS,
            diff_flags: WORKTREE_DIFF_FLAGS,
            environment,
            tracked_diff,
            untracked_list,
            untracked_diff,
            description,
        }
    }
}

fn pathspec() -> impl Iterator<Item = String> {
    ["--".to_owned(), ".".to_owned()]
        .into_iter()
        .chain(WORKTREE_DIFF_EXCLUSIONS.map(|path| format!(":(exclude){path}")))
}

fn tracked_diff_args() -> Vec<String> {
    // GIT_OPTIONAL_LOCKS=0 alone does not stop porcelain diff from refreshing
    // stale index stat data (notably after a fixture repository is copied).
    let mut args = vec!["-c".to_owned(), "diff.autoRefreshIndex=false".to_owned(), "diff".to_owned()];
    args.extend(WORKTREE_DIFF_FLAGS.map(str::to_owned));
    args.push("HEAD".to_owned());
    args.extend(pathspec());
    args
}

fn untracked_list_args() -> Vec<String> {
    let mut args = Vec::from(["ls-files", "--others", "--exclude-standard", "-z"].map(str::to_owned));
    args.extend(pathspec());
    args
}

fn untracked_diff_args(path: &str) -> Vec<String> {
    let mut args = vec!["-c".to_owned(), "diff.autoRefreshIndex=false".to_owned(), "diff".to_owned()];
    args.extend(WORKTREE_DIFF_FLAGS.map(str::to_owned));
    args.extend(["--no-index", "--", "/dev/null", path].map(str::to_owned));
    args
}

/// Code version of a git working tree under [`WORKTREE_DIFF_RULE`].
pub struct WorktreeDiff {
    pub repo_root: PathBuf,
    pub head: String,
    /// `false` exactly when `bytes` is empty.
    pub dirty: bool,
    /// SHA-256 of `bytes`; `None` when the tree is clean.
    pub sha256: Option<String>,
    /// The complete diff: part 1 then part 2, exactly as git wrote it.
    pub bytes: Vec<u8>,
    /// Length of part 1 (the tracked diff) at the start of `bytes`.
    pub tracked_bytes: usize,
    /// Untracked files of part 2 in the order they were appended.
    pub untracked_files: Vec<String>,
    pub rule: DiffRule,
}

impl WorktreeDiff {
    /// Save the complete diff byte for byte; never overwrites (see [`write_new`]).
    pub fn save(&self, path: &Path, credentials: &Credentials) -> Result<()> {
        write_new(path, &self.bytes, credentials)
    }
}

/// Compute the code version of the repository containing `dir`. Fails when
/// `dir` is not inside a git working tree or HEAD has no commit.
pub fn worktree_diff(dir: &Path) -> Result<WorktreeDiff> {
    let top = git(dir, &["rev-parse", "--show-toplevel"], &[0])
        .with_context(|| format!("cannot determine the git repository root of {}", dir.display()))?;
    let top = String::from_utf8(top).context("git repository root is not valid UTF-8")?;
    let repo_root = PathBuf::from(top.trim_end_matches(['\r', '\n']));
    let head = git(&repo_root, &["rev-parse", "--verify", "HEAD^{commit}"], &[0])
        .with_context(|| format!("cannot determine the HEAD commit of {}", repo_root.display()))?;
    let head = String::from_utf8(head).context("HEAD commit id is not valid UTF-8")?.trim().to_owned();
    ensure!(
        !head.is_empty() && head.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "unexpected HEAD commit id `{head}` in {}",
        repo_root.display()
    );

    let mut bytes = git(&repo_root, &tracked_diff_args(), &[0])?;
    let tracked_bytes = bytes.len();
    let listing = git(&repo_root, &untracked_list_args(), &[0])?;
    let mut paths: Vec<&[u8]> = listing.split(|byte| *byte == 0).filter(|path| !path.is_empty()).collect();
    paths.sort_unstable();
    let untracked_files = paths
        .into_iter()
        .map(|path| String::from_utf8(path.to_vec()).context("git listed an untracked path that is not valid UTF-8"))
        .collect::<Result<Vec<_>>>()?;
    for path in &untracked_files {
        bytes.extend(git(&repo_root, &untracked_diff_args(path), &[0, 1])?);
    }

    let sha256 = (!bytes.is_empty()).then(|| sha256_hex(&bytes));
    Ok(WorktreeDiff {
        repo_root,
        head,
        dirty: sha256.is_some(),
        sha256,
        bytes,
        tracked_bytes,
        untracked_files,
        rule: DiffRule::current(),
    })
}

/// Raw stdout of one git call; any exit code outside `ok_codes` is an error.
fn git<S: AsRef<str>>(dir: &Path, args: &[S], ok_codes: &[i32]) -> Result<Vec<u8>> {
    let args: Vec<&str> = args.iter().map(AsRef::as_ref).collect();
    let output = Command::new("git")
        .current_dir(dir)
        .env(GIT_ENVIRONMENT.0, GIT_ENVIRONMENT.1)
        .args(&args)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("cannot run `git {}` in {} (is git on PATH?)", args.join(" "), dir.display()))?;
    match output.status.code() {
        Some(code) if ok_codes.contains(&code) => Ok(output.stdout),
        code => bail!(
            "`git {}` in {} exited with {}: {}",
            args.join(" "),
            dir.display(),
            code.map_or_else(|| "no exit code".to_owned(), |code| code.to_string()),
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVIDENCE: &str = "crates/ara-agent-lite-core/docs/evidence/lite-capability";

    fn write(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn entry_line(path: &str, bytes: &[u8]) -> String {
        format!("{path}\0{}\0{}\n", bytes.len(), sha256_hex(bytes))
    }

    #[test]
    fn tree_digest_hashes_the_documented_concatenation_in_utf8_byte_order() {
        let dir = tempfile::tempdir().unwrap();
        // U+FF61 sorts before U+1F600 in UTF-8 bytes but after it in UTF-16 units.
        let files: [(&str, &[u8]); 6] = [
            ("A.txt", b"upper\n"),
            ("a/b c.txt", b"space\r\n"),
            ("a/z.bin", b"\0\xff\0"),
            ("b.txt", b""),
            ("\u{ff61}.txt", "halfwidth".as_bytes()),
            ("\u{1f600}.txt", "emoji".as_bytes()),
        ];
        for (path, bytes) in files.iter().rev() {
            write(&dir.path().join(path), bytes);
        }
        fs::create_dir_all(dir.path().join("empty/nested")).unwrap();

        let digest = tree_digest(dir.path()).unwrap();
        let expected: String = files.iter().map(|(path, bytes)| entry_line(path, bytes)).collect();
        assert_eq!(digest.rule, "lite-eval-tree-v1");
        assert_eq!(digest.sha256, sha256_hex(expected.as_bytes()));
        let paths: Vec<_> = digest.entries.iter().map(|entry| entry.path.as_str()).collect();
        assert_eq!(paths, files.map(|(path, _)| path));
        assert_eq!(digest.entries[2], TreeEntry { path: "a/z.bin".into(), size: 3, sha256: sha256_hex(b"\0\xff\0") });
    }

    #[test]
    fn tree_digest_reacts_to_content_and_path_changes() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("src/main.cpp"), b"int main() {}\n");
        let before = tree_digest(dir.path()).unwrap().sha256;
        assert_eq!(tree_digest(dir.path()).unwrap().sha256, before);

        write(&dir.path().join("src/main.cpp"), b"int main() {}\r\n");
        let content_changed = tree_digest(dir.path()).unwrap().sha256;
        assert_ne!(content_changed, before);

        fs::rename(dir.path().join("src/main.cpp"), dir.path().join("src/app.cpp")).unwrap();
        let renamed = tree_digest(dir.path()).unwrap().sha256;
        assert_ne!(renamed, content_changed);
    }

    #[test]
    fn a_single_file_is_a_one_entry_tree_named_after_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("only/fixture.cpp");
        write(&file, b"// fixture\n");
        let single = tree_digest(&file).unwrap();
        assert_eq!(single, tree_digest(&dir.path().join("only")).unwrap());
        assert_eq!(single.sha256, sha256_hex(entry_line("fixture.cpp", b"// fixture\n").as_bytes()));
        assert!(tree_digest(&dir.path().join("missing")).is_err());
    }

    #[cfg(windows)]
    fn make_dir_link(target: &Path, link: &Path) {
        // Junctions need no privilege, unlike symbolic links on Windows.
        let status = Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "mklink /J {} {} failed", link.display(), target.display());
    }

    #[cfg(unix)]
    fn make_dir_link(target: &Path, link: &Path) {
        std::os::unix::fs::symlink(target, link).unwrap();
    }

    #[test]
    fn links_inside_or_at_the_root_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside");
        write(&outside.join("oracle.txt"), b"secret\n");
        let tree = dir.path().join("tree");
        write(&tree.join("a.txt"), b"a\n");
        make_dir_link(&outside, &tree.join("linked"));
        let error = format!("{:#}", tree_digest(&tree).unwrap_err());
        assert!(error.contains("reparse point") && error.contains("linked"), "{error}");

        let root_link = dir.path().join("root-link");
        make_dir_link(&outside, &root_link);
        let error = format!("{:#}", tree_digest(&root_link).unwrap_err());
        assert!(error.contains("reparse point"), "{error}");
    }

    fn run_git(dir: &Path, args: &[&str]) -> Vec<u8> {
        let output = Command::new("git").current_dir(dir).args(args).output().unwrap();
        assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
        output.stdout
    }

    fn temp_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        run_git(dir.path(), &["-c", "init.defaultBranch=main", "init", "-q"]);
        for (key, value) in [
            ("core.autocrlf", "false"),
            ("user.name", "lite-eval test"),
            ("user.email", "lite-eval@example.invalid"),
            ("commit.gpgsign", "false"),
        ] {
            run_git(dir.path(), &["config", key, value]);
        }
        dir
    }

    fn commit_all(root: &Path) {
        run_git(root, &["add", "--all"]);
        run_git(root, &["commit", "-q", "-m", "fixture"]);
    }

    fn position(haystack: &str, needle: &str) -> usize {
        haystack.find(needle).unwrap_or_else(|| panic!("`{needle}` not in diff:\n{haystack}"))
    }

    #[test]
    fn diff_rule_states_exclusions_and_flags() {
        let rule = DiffRule::current();
        let value = serde_json::to_value(&rule).unwrap();
        assert_eq!(value["id"], "lite-eval-worktree-diff-v1");
        assert_eq!(value["exclusions"], serde_json::json!([EVIDENCE, ".codebase-memory"]));
        assert_eq!(value["diff_flags"], serde_json::json!(["--binary", "--no-textconv", "--no-ext-diff"]));
        assert_eq!(
            rule.tracked_diff,
            format!("git -c diff.autoRefreshIndex=false diff --binary --no-textconv --no-ext-diff HEAD -- . :(exclude){EVIDENCE} :(exclude).codebase-memory")
        );
        assert_eq!(rule.untracked_diff, "git -c diff.autoRefreshIndex=false diff --binary --no-textconv --no-ext-diff --no-index -- /dev/null <path>");
        for needle in [
            &format!("{EVIDENCE}/"),
            ".codebase-memory/",
            "--no-textconv",
            "--no-ext-diff",
            "GIT_OPTIONAL_LOCKS=0",
            "diff.autoRefreshIndex=false",
        ] {
            assert!(rule.description.contains(needle), "{needle} missing from {}", rule.description);
        }
    }

    #[test]
    fn clean_tree_and_excluded_changes_are_not_dirty() {
        let repo = temp_repo();
        let root = repo.path();
        write(&root.join(".gitignore"), b"/ignored.log\n");
        write(&root.join("src/lib.rs"), b"pub fn f() {}\n");
        write(&root.join(EVIDENCE).join("progress-ledger.jsonl"), b"{\"seq\":1}\n");
        commit_all(root);
        write(&root.join("ignored.log"), b"ignored\n");
        write(&root.join(EVIDENCE).join("progress-ledger.jsonl"), b"{\"seq\":1}\n{\"seq\":2}\n");
        write(&root.join(EVIDENCE).join("gates/new.json"), b"{}\n");
        write(&root.join(".codebase-memory/graph.db.zst"), b"tool cache\n");

        let diff = worktree_diff(root).unwrap();
        assert!(!diff.dirty);
        assert_eq!(diff.sha256, None);
        assert!(diff.bytes.is_empty());
        assert!(diff.untracked_files.is_empty());
        assert_eq!(diff.head, String::from_utf8(run_git(root, &["rev-parse", "HEAD"])).unwrap().trim());
        assert_eq!(fs::canonicalize(&diff.repo_root).unwrap(), fs::canonicalize(root).unwrap());
    }

    #[test]
    fn diff_covers_tracked_and_untracked_changes_as_raw_bytes_without_touching_the_index() {
        let repo = temp_repo();
        let root = repo.path();
        write(&root.join(".gitattributes"), b"*.txt diff=lite-eval-upper\n");
        write(&root.join("tracked.txt"), b"one\n");
        write(&root.join("staged.txt"), b"two\n");
        write(&root.join(EVIDENCE).join("ledger.jsonl"), b"tracked evidence\n");
        commit_all(root);
        // Both drivers point at missing programs: git fails unless the rule bypasses them.
        run_git(root, &["config", "diff.lite-eval-upper.textconv", "lite-eval-missing-textconv"]);
        run_git(root, &["config", "diff.external", "lite-eval-missing-external-diff"]);
        write(&root.join("tracked.txt"), b"one\nunstaged line\n");
        write(&root.join("staged.txt"), b"two\nstaged line\n");
        run_git(root, &["add", "staged.txt"]);
        for flags in [&["--binary"][..], &["--binary", "--no-ext-diff"]] {
            let output = Command::new("git").current_dir(root).arg("diff").args(flags).arg("HEAD").output().unwrap();
            assert!(!output.status.success(), "test setup: git diff {flags:?} must hit a missing driver");
        }

        write(&root.join(EVIDENCE).join("ledger.jsonl"), b"tracked evidence\nappended\n");
        write(&root.join(EVIDENCE).join("new.json"), b"untracked evidence\n");
        write(&root.join(".codebase-memory/graph.db.zst"), b"tool cache\n");
        write(&root.join("z.txt"), b"untracked z\n");
        write(&root.join("b/a.txt"), b"untracked b/a\n");
        write(&root.join("B.txt"), b"untracked B\n");
        write(&root.join("blob.bin"), b"\0\x01binary\xff");
        let index = fs::read(root.join(".git/index")).unwrap();

        let diff = worktree_diff(&root.join("b")).unwrap();
        assert_eq!(fs::read(root.join(".git/index")).unwrap(), index, "the index must not be modified");
        assert_eq!(diff.head, String::from_utf8(run_git(root, &["rev-parse", "HEAD"])).unwrap().trim());
        assert_eq!(diff.untracked_files, ["B.txt", "b/a.txt", "blob.bin", "z.txt"]);
        assert!(diff.dirty);
        assert_eq!(diff.sha256.as_deref(), Some(sha256_hex(&diff.bytes).as_str()));

        let text = String::from_utf8_lossy(&diff.bytes);
        let tracked = String::from_utf8_lossy(&diff.bytes[..diff.tracked_bytes]);
        assert!(tracked.contains("+unstaged line") && tracked.contains("+staged line"), "{tracked}");
        assert!(!tracked.contains("untracked"), "{tracked}");
        let order = ["+untracked B", "+untracked b/a", "GIT binary patch", "+untracked z"]
            .map(|needle| position(&text, needle));
        assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{order:?}");
        for excluded in ["evidence", "appended", "tool cache", ".codebase-memory"] {
            assert!(!text.contains(excluded), "`{excluded}` leaked into the diff:\n{text}");
        }

        assert_eq!(worktree_diff(root).unwrap().bytes, diff.bytes, "same tree, same bytes");
        let out = tempfile::tempdir().unwrap();
        let saved = out.path().join("worktree.diff");
        diff.save(&saved, &Credentials::default()).unwrap();
        assert_eq!(fs::read(&saved).unwrap(), diff.bytes);
        assert!(diff.save(&saved, &Credentials::default()).is_err(), "saving never overwrites");
    }

    #[test]
    fn missing_repository_or_head_commit_is_an_error() {
        let plain = tempfile::tempdir().unwrap();
        let Err(error) = worktree_diff(plain.path()) else { panic!("a plain directory has no code version") };
        assert!(format!("{error:#}").contains("repository root"), "{error:#}");

        let unborn = temp_repo();
        write(&unborn.path().join("a.txt"), b"a\n");
        let Err(error) = worktree_diff(unborn.path()) else { panic!("a repository without commits has no HEAD") };
        assert!(format!("{error:#}").contains("HEAD commit"), "{error:#}");
    }
}
