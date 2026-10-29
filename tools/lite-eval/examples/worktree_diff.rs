//! Compute the `lite-eval-worktree-diff-v1` code version of a repository and
//! print it as JSON; with a second argument, also save the complete diff
//! there (the file must not exist yet).
//!
//! ```text
//! cargo run --example worktree_diff -- <dir-inside-repo> [<save-path>]
//! ```

use std::{env, path::PathBuf};

use anyhow::{ensure, Context, Result};
use lite_eval::{
    common::{sha256_file, sha256_hex, timestamp_now, Credentials},
    digest::worktree_diff,
};

fn main() -> Result<()> {
    let mut args = env::args_os().skip(1).map(PathBuf::from);
    let repo = args.next().context("usage: worktree_diff <dir-inside-repo> [<save-path>]")?;
    let save = args.next();

    let started_at = timestamp_now();
    let diff = worktree_diff(&repo)?;
    let finished_at = timestamp_now();
    if let Some(path) = &save {
        diff.save(path, &Credentials::default())?;
        ensure!(
            sha256_file(path)? == sha256_hex(&diff.bytes),
            "saved diff {} differs from the computed one",
            path.display()
        );
    }

    let summary = serde_json::json!({
        "repo_root": diff.repo_root,
        "head": diff.head,
        "dirty": diff.dirty,
        "worktree_diff_sha256": diff.sha256,
        "total_bytes": diff.bytes.len(),
        "tracked_diff_bytes": diff.tracked_bytes,
        "untracked_files_included": diff.untracked_files.len(),
        "diff_path": save,
        "diff_rule": diff.rule,
        "started_at": started_at,
        "finished_at": finished_at,
    });
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}
