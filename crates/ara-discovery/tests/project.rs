//! End-to-end context assembly through `Discovery::load_project_context_files`.

use ara_discovery::{Discovery, HostDirs, Level, LoadOptions, ProviderPolicy};
use std::fs;
use std::path::{Path, PathBuf};

fn write(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

struct Tree {
    _dir: tempfile::TempDir,
    home: PathBuf,
    repo: PathBuf,
    cwd: PathBuf,
}

fn tree() -> Tree {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let home = root.join("home");
    let repo = home.join("code/mono");
    let cwd = repo.join("packages/app");
    fs::create_dir_all(repo.join(".git")).unwrap();
    fs::create_dir_all(&cwd).unwrap();
    Tree { _dir: dir, home, repo, cwd }
}

#[test]
fn monorepo_context_is_ordered_expanded_deduplicated_and_sourced() {
    let t = tree();
    write(&t.home.join(".ara/agent/AGENTS.md"), "User rule: be terse.");
    write(&t.repo.join("AGENTS.md"), "Repo rule one.\n\nRepo rule two.");
    write(&t.repo.join("docs/style.md"), "Style: tabs.");
    write(&t.repo.join("packages/AGENTS.md"), "Packages rule.\n\nSee @../docs/style.md");
    // The cwd file imports the repo file, so the repo file is contained and dropped.
    write(&t.cwd.join("CLAUDE.md"), "@../../AGENTS.md\n\nApp rule.");
    write(&t.home.join("AGENTS.md"), "home file must not load inside a nested repo");

    let discovery = Discovery::new(&t.home, HostDirs::ara(&t.home), ProviderPolicy::default());
    let files = discovery.load_project_context_files(&t.cwd, &[]);
    let summary: Vec<(PathBuf, Option<i64>, String)> =
        files.iter().map(|f| (f.path.clone(), f.depth, f.source.provider.clone())).collect();
    assert_eq!(
        summary,
        [
            (t.home.join(".ara/agent/AGENTS.md"), None, "native".into()),
            (t.repo.join("packages/AGENTS.md"), Some(1), "agents-md".into()),
            (t.cwd.join("CLAUDE.md"), Some(0), "claude-md".into()),
        ]
    );
    assert_eq!(files[1].content, "Packages rule.\n\nSee Style: tabs.");
    assert_eq!(files[2].content, "Repo rule one.\n\nRepo rule two.\n\nApp rule.");
    assert_eq!(files[0].source.level, Level::User);
    assert_eq!(files[2].source.provider_name, "CLAUDE.md");
}

#[test]
fn foreign_user_config_is_opt_in_but_agents_dirs_are_not() {
    let t = tree();
    write(&t.home.join(".claude/CLAUDE.md"), "claude user");
    write(&t.home.join(".codex/AGENTS.md"), "codex user");
    write(&t.home.join(".agents/AGENTS.md"), "agents user");
    let user_files = |discovery: &Discovery| -> Vec<String> {
        let result = discovery.load_context_files(&t.cwd, &LoadOptions::default());
        result.all.iter().filter(|(f, _)| f.level == Level::User).map(|(f, _)| f.content.clone()).collect()
    };

    let default = Discovery::new(&t.home, HostDirs::ara(&t.home), ProviderPolicy::default());
    assert_eq!(user_files(&default), ["agents user"]);

    let mut policy = ProviderPolicy::default();
    policy.enabled_user_sources.insert("claude".into());
    let opted_in = Discovery::new(&t.home, HostDirs::ara(&t.home), policy);
    assert_eq!(user_files(&opted_in), ["claude user", "agents user"]);
    // One user-level file survives: the highest-priority provider's.
    let items = opted_in.load_context_files(&t.cwd, &LoadOptions::default()).items;
    assert_eq!(items.iter().filter(|f| f.level == Level::User).count(), 1);
    assert_eq!(items.iter().find(|f| f.level == Level::User).unwrap().content, "claude user");

    let mut dirs = HostDirs::ara(&t.home);
    dirs.claude_config_dir = Some(t.home.join("alt-claude"));
    write(&t.home.join("alt-claude/CLAUDE.md"), "relocated claude");
    let relocated = Discovery::new(&t.home, dirs, ProviderPolicy::default());
    assert_eq!(user_files(&relocated), ["relocated claude", "agents user"]);

    let mut policy = ProviderPolicy::default();
    policy.enabled_user_sources.insert("*".into());
    policy.disabled.insert("claude".into());
    let all_but_claude = Discovery::new(&t.home, HostDirs::ara(&t.home), policy);
    assert_eq!(user_files(&all_but_claude), ["agents user", "codex user"]);
}

#[test]
fn native_project_dir_is_the_nearest_non_empty_one_up_to_the_repo_root() {
    let t = tree();
    write(&t.repo.join(".ara/AGENTS.md"), "repo native");
    fs::create_dir_all(t.repo.join("packages/.ara")).unwrap(); // empty: skipped
    let discovery = Discovery::new(&t.home, HostDirs::ara(&t.home), ProviderPolicy::default());
    let items = discovery.load_context_files(&t.cwd, &LoadOptions::default()).items;
    let native: Vec<_> = items.iter().filter(|f| f.source.provider == "native").collect();
    assert_eq!(native.len(), 1);
    assert_eq!((native[0].path.clone(), native[0].depth), (t.repo.join(".ara/AGENTS.md"), Some(2)));
    assert_eq!(native[0].source.provider_name, "ARA");
}

#[test]
fn cache_serves_repeat_reads_until_invalidated() {
    let t = tree();
    let file = t.cwd.join("AGENTS.md");
    write(&file, "v1");
    let discovery = Discovery::new(&t.home, HostDirs::ara(&t.home), ProviderPolicy::default());
    let content = |d: &Discovery| d.load_project_context_files(&t.cwd, &[]).last().map(|f| f.content.clone());
    assert_eq!(content(&discovery).as_deref(), Some("v1"));
    write(&file, "v2");
    assert_eq!(content(&discovery).as_deref(), Some("v1"));
    discovery.fs.invalidate(&file);
    assert_eq!(content(&discovery).as_deref(), Some("v2"));
}

#[test]
fn bom_and_invalid_utf8_decode_like_bun() {
    let t = tree();
    fs::write(t.cwd.join("AGENTS.md"), b"\xEF\xBB\xBFrule \xFF end").unwrap();
    let discovery = Discovery::new(&t.home, HostDirs::ara(&t.home), ProviderPolicy::default());
    let files = discovery.load_project_context_files(&t.cwd, &[]);
    assert_eq!(files.last().unwrap().content, "rule \u{FFFD} end");
}

#[test]
fn with_env_reads_foreign_overrides_and_wsl_home() {
    let env = |k: &str| match k {
        "CLAUDE_CONFIG_DIR" => Some("  /cfg/claude/../claude2  ".to_string()),
        "COPILOT_HOME" => Some("/cop".to_string()),
        "COPILOT_CUSTOM_INSTRUCTIONS_DIRS" => Some(" /a, ,/b ".to_string()),
        "WSL_DISTRO_NAME" => Some("Ubuntu".to_string()),
        "USERPROFILE" => Some(r"C:\Users\Me".to_string()),
        _ => None,
    };
    let dirs = HostDirs::ara(Path::new("/h")).with_env(env);
    assert_eq!(dirs.claude_config_dir, Some(PathBuf::from("/cfg/claude2")));
    assert_eq!(dirs.copilot_home, Some(PathBuf::from("/cop")));
    assert_eq!(dirs.copilot_custom_instruction_dirs, [PathBuf::from("/a"), PathBuf::from("/b")]);
    assert_eq!(dirs.extra_user_homes, [PathBuf::from("/mnt/c/Users/Me")]);
}
