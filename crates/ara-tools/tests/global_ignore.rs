//! The global gitignore applies only inside a repository (pi-walker
//! `FastIgnore::is_ignored`). Own test binary: it sets HOME/XDG_CONFIG_HOME.

use ara_agent::AgentTool;
use ara_tools::ToolContext;
use ara_tools::grep::GrepTool;
use serde_json::json;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn global_gitignore_needs_a_repository() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join("git")).unwrap();
    std::fs::write(home.path().join("git/ignore"), "*.log\n").unwrap();
    // SAFETY: this binary has a single test, so no other thread reads the environment.
    unsafe {
        std::env::set_var("HOME", home.path());
        std::env::set_var("XDG_CONFIG_HOME", home.path());
    }
    let run = |dir: std::path::PathBuf| async move {
        let args = json!({"pattern": "token"}).as_object().unwrap().clone();
        let out = GrepTool::new(ToolContext::new(dir))
            .execute("c", args, CancellationToken::new(), Arc::new(|_| {}))
            .await
            .unwrap();
        out.details.unwrap()["files"].clone()
    };
    let plain = tempfile::tempdir().unwrap();
    std::fs::write(plain.path().join("a.log"), "token\n").unwrap();
    assert_eq!(run(plain.path().to_path_buf()).await, json!(["a.log"]));
    let repo = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(repo.path().join(".git")).unwrap();
    std::fs::write(repo.path().join("a.log"), "token\n").unwrap();
    std::fs::write(repo.path().join("b.txt"), "token\n").unwrap();
    assert_eq!(run(repo.path().to_path_buf()).await, json!(["b.txt"]));
}
