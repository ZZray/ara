//! Print the `lite-eval-tree-v1` digest of each fixture path as JSON, for
//! registering `fixture_sha256` in the Task_Set.
//!
//! ```text
//! cargo run --example fixture_digest -- <fixture-path>...
//! ```

use std::{env, path::PathBuf};

use anyhow::{ensure, Result};
use lite_eval::digest::tree_digest;

fn main() -> Result<()> {
    let paths: Vec<PathBuf> = env::args_os().skip(1).map(PathBuf::from).collect();
    ensure!(!paths.is_empty(), "usage: fixture_digest <fixture-path>...");
    for path in paths {
        let digest = tree_digest(&path)?;
        let summary = serde_json::json!({
            "fixture_path": path,
            "rule": digest.rule,
            "sha256": digest.sha256,
            "files": digest.entries.len(),
        });
        println!("{summary}");
    }
    Ok(())
}
