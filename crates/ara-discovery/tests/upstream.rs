//! Ports of OMP coding-agent tests at 596f2da7101178214aa27a753529d15e6b7ad91d:
//! `test/discovery/{agents-md,claude-md,at-imports,context-file-dedup,
//! github-copilot,disabled-extensions}.test.ts`,
//! `test/capability/fs-special-files.test.ts` and
//! `test/system-prompt-context-dedup.test.ts`. Each test names its
//! inventory behavior IDs.

use ara_discovery::{
    ContextFile, Discovery, Expander, FsCache, HostDirs, Level, LoadContext, LoadOptions, MAX_AT_IMPORT_DEPTH,
    ProjectContextFile, ProviderPolicy, SourceMeta, dedupe_contained_context_files, load_standalone_context_files,
};
use std::fs;
use std::path::{Path, PathBuf};

fn write(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn canonical_tempdir() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    (dir, root)
}

fn standalone(cwd: &Path, home: &Path, repo_root: Option<&Path>, file_name: &str) -> Vec<PathBuf> {
    let dirs = HostDirs::ara(home);
    let policy = ProviderPolicy::default();
    let fs = FsCache::new();
    let ctx = LoadContext {
        cwd: cwd.to_path_buf(),
        home: home.to_path_buf(),
        repo_root: repo_root.map(Path::to_path_buf),
        explicit_providers: None,
        include_opt_out_user_sources: false,
        dirs: &dirs,
        policy: &policy,
        fs: &fs,
    };
    load_standalone_context_files(&ctx, "test", file_name).into_iter().map(|f| f.path).collect()
}

// --- agents-md.test.ts / claude-md.test.ts (standalone walker) -----------------

/// B-f64db7b8bd, B-2500482746
#[test]
fn workspace_file_above_nested_repo_without_home_context() {
    for name in ["AGENTS.md", "CLAUDE.md"] {
        let (_t, tmp) = canonical_tempdir();
        let home = tmp.join("home");
        let workspace = home.join("repos/writer");
        let repo = workspace.join("internal/service");
        let cwd = repo.join("src");
        fs::create_dir_all(&cwd).unwrap();
        write(&repo.join(name), "repo context");
        write(&workspace.join(name), "workspace context");
        write(&home.join(name), "home context");
        assert_eq!(standalone(&cwd, &home, Some(&repo), name), [repo.join(name), workspace.join(name)]);
    }
}

/// B-1de451283d, B-90ccaacc32
#[test]
fn cwd_and_intermediate_context_without_repo_under_home() {
    for name in ["AGENTS.md", "CLAUDE.md"] {
        let (_t, tmp) = canonical_tempdir();
        let home = tmp.join("home");
        let intermediate = home.join("workspace/packages");
        let cwd = intermediate.join("service");
        fs::create_dir_all(&cwd).unwrap();
        write(&cwd.join(name), "cwd context");
        write(&intermediate.join(name), "intermediate context");
        write(&home.join(name), "home context");
        assert_eq!(standalone(&cwd, &home, None, name), [cwd.join(name), intermediate.join(name), home.join(name)]);
    }
}

/// B-3a467968c9, B-e410a9f158
#[test]
fn home_context_when_repo_root_is_above_home() {
    for name in ["AGENTS.md", "CLAUDE.md"] {
        let (_t, tmp) = canonical_tempdir();
        let repo = tmp.join("workspace");
        let home = repo.join("user");
        let cwd = home.join("project");
        fs::create_dir_all(&cwd).unwrap();
        write(&repo.join(name), "repo context");
        write(&home.join(name), "home context");
        assert_eq!(standalone(&cwd, &home, Some(&repo), name), [home.join(name), repo.join(name)]);
    }
}

/// B-8aa58d2c8a, B-00fdd836bb
#[test]
fn repo_root_boundary_when_repo_is_outside_home() {
    for name in ["AGENTS.md", "CLAUDE.md"] {
        let (_t, tmp) = canonical_tempdir();
        let home = tmp.join("home");
        let workspace = tmp.join("workspace");
        let repo = workspace.join("service");
        let cwd = repo.join("src");
        fs::create_dir_all(&cwd).unwrap();
        write(&repo.join(name), "repo context");
        write(&workspace.join(name), "workspace context");
        assert_eq!(standalone(&cwd, &home, Some(&repo), name), [repo.join(name)]);
    }
}

/// B-87d239ae9e, B-4f6353345d
#[test]
fn skips_file_inside_hidden_owner_directory() {
    for name in ["AGENTS.md", "CLAUDE.md"] {
        let (_t, tmp) = canonical_tempdir();
        let home = tmp.join("home");
        let repo = home.join("repo");
        let hidden = repo.join(".hidden");
        let cwd = hidden.join("service");
        fs::create_dir_all(&cwd).unwrap();
        write(&hidden.join(name), "hidden context");
        write(&repo.join(name), "repo context");
        assert_eq!(standalone(&cwd, &home, Some(&repo), name), [repo.join(name)]);
    }
}

/// B-2d4f2a2511
#[test]
fn repo_root_file_when_repo_root_is_home() {
    let (_t, tmp) = canonical_tempdir();
    let home = tmp.join("home");
    let cwd = home.join("project");
    fs::create_dir_all(&cwd).unwrap();
    write(&home.join("CLAUDE.md"), "repo root context");
    assert_eq!(standalone(&cwd, &home, Some(&home), "CLAUDE.md"), [home.join("CLAUDE.md")]);
}

// --- claude-md.test.ts (registration and precedence) ---------------------------

fn repo_with_src() -> (tempfile::TempDir, PathBuf, PathBuf, Discovery) {
    let (t, tmp) = canonical_tempdir();
    let repo = tmp.join("repo");
    let cwd = repo.join("src");
    fs::create_dir_all(repo.join(".git")).unwrap();
    fs::create_dir_all(&cwd).unwrap();
    let home = tmp.join("home");
    fs::create_dir_all(&home).unwrap();
    let discovery = Discovery::new(&home, HostDirs::ara(&home), ProviderPolicy::default());
    (t, repo, cwd, discovery)
}

fn project_at_depth(files: &[ContextFile], depth: i64) -> Vec<PathBuf> {
    files.iter().filter(|f| f.level == Level::Project && f.depth == Some(depth)).map(|f| f.path.clone()).collect()
}

/// B-d9b1c3454d
#[test]
fn standalone_claude_md_through_the_capability() {
    let (_t, repo, cwd, discovery) = repo_with_src();
    write(&repo.join("CLAUDE.md"), "root context");
    write(&cwd.join("CLAUDE.md"), "cwd context");
    let result = discovery.load_context_files(&cwd, &LoadOptions::default());
    let claude: Vec<_> =
        result.items.iter().filter(|f| f.source.provider_name == "CLAUDE.md").map(|f| f.path.clone()).collect();
    assert_eq!(claude, [cwd.join("CLAUDE.md"), repo.join("CLAUDE.md")]);
}

/// B-38509e4005
#[test]
fn dot_claude_shadows_standalone_at_same_depth() {
    let (_t, _repo, cwd, discovery) = repo_with_src();
    write(&cwd.join(".claude/CLAUDE.md"), "config-dir context");
    write(&cwd.join("CLAUDE.md"), "standalone context");
    let result = discovery.load_context_files(&cwd, &LoadOptions::default());
    assert_eq!(project_at_depth(&result.items, 0), [cwd.join(".claude/CLAUDE.md")]);
    assert!(result.all.iter().any(|(f, shadowed)| f.path == cwd.join("CLAUDE.md") && *shadowed));
}

/// B-3a8e5af2fb
#[test]
fn standalone_agents_md_wins_the_depth_tie() {
    let (_t, repo, cwd, discovery) = repo_with_src();
    write(&repo.join("CLAUDE.md"), "claude context");
    write(&repo.join("AGENTS.md"), "agents context");
    let result = discovery.load_context_files(&cwd, &LoadOptions::default());
    assert_eq!(project_at_depth(&result.items, 1), [repo.join("AGENTS.md")]);
}

/// B-ecf4163a6a
#[test]
fn empty_standalone_files_do_not_claim_a_depth() {
    let (_t, repo, cwd, discovery) = repo_with_src();
    write(&repo.join("AGENTS.md"), "");
    write(&repo.join("CLAUDE.md"), "claude context");
    let result = discovery.load_context_files(&cwd, &LoadOptions::default());
    assert_eq!(project_at_depth(&result.items, 1), [repo.join("CLAUDE.md")]);
}

// --- disabled-extensions.test.ts -----------------------------------------------

/// B-fedc015393, B-3d03fa0ebb (native project dir is `.ara` in ARA, `.omp` upstream)
#[test]
fn disabled_context_files_hidden_unless_included() {
    let (_t, tmp) = canonical_tempdir();
    let home = tmp.join("home");
    let cwd = tmp.join("project");
    fs::create_dir_all(&home).unwrap();
    write(&cwd.join(".ara/AGENTS.md"), "# project instructions\n");
    let discovery = Discovery::new(&home, HostDirs::ara(&home), ProviderPolicy::default());
    let disabled = vec!["context-file:project:AGENTS.md".to_string()];
    let hidden = discovery
        .load_context_files(&cwd, &LoadOptions { disabled_extensions: disabled.clone(), ..LoadOptions::default() });
    assert!(hidden.items.is_empty());
    let shown = discovery.load_context_files(
        &cwd,
        &LoadOptions { disabled_extensions: disabled, include_disabled: true, ..LoadOptions::default() },
    );
    assert_eq!(shown.items.len(), 1);
    assert_eq!(shown.items[0].path.file_name().unwrap(), "AGENTS.md");
}

// --- github-copilot.test.ts (context-file cases) ---------------------------------

fn copilot(tmp: &Path, custom: Vec<PathBuf>) -> (Discovery, PathBuf, PathBuf) {
    let home = tmp.join("home");
    let cwd = tmp.join("project");
    let copilot_home = tmp.join("copilot-home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    let mut dirs = HostDirs::ara(&home);
    dirs.copilot_home = Some(copilot_home.clone());
    dirs.copilot_custom_instruction_dirs = custom;
    (Discovery::new(&home, dirs, ProviderPolicy::default()), cwd, copilot_home)
}

fn github_only() -> LoadOptions<'static, ContextFile> {
    LoadOptions { providers: Some(vec!["github".into()]), ..LoadOptions::default() }
}

/// B-23b2225708
#[test]
fn copilot_user_global_instructions_via_copilot_home() {
    let (_t, tmp) = canonical_tempdir();
    let (discovery, cwd, copilot_home) = copilot(&tmp, Vec::new());
    write(&copilot_home.join("copilot-instructions.md"), "user-global guidance");
    let result = discovery.load_context_files(&cwd, &github_only());
    let found =
        result.all.iter().map(|(f, _)| f).find(|f| f.path == copilot_home.join("copilot-instructions.md")).unwrap();
    assert_eq!(found.content, "user-global guidance");
    assert_eq!(found.level, Level::User);
    assert_eq!(found.source.provider, "github");
}

/// B-6faff7402b
#[test]
fn copilot_project_and_user_instructions_together() {
    let (_t, tmp) = canonical_tempdir();
    let (discovery, cwd, copilot_home) = copilot(&tmp, Vec::new());
    write(&cwd.join(".github/copilot-instructions.md"), "project guidance");
    write(&copilot_home.join("copilot-instructions.md"), "user guidance");
    let result = discovery.load_context_files(&cwd, &github_only());
    let by_level = |level| result.all.iter().map(|(f, _)| f).find(|f| f.level == level).map(|f| f.content.clone());
    assert_eq!(by_level(Level::Project).as_deref(), Some("project guidance"));
    assert_eq!(by_level(Level::User).as_deref(), Some("user guidance"));
}

/// B-6cce486e5a (the directory list is parsed by `HostDirs::with_env`)
#[test]
fn copilot_custom_instruction_dirs_contribute_agents_md() {
    let (_t, tmp) = canonical_tempdir();
    let extra_a = tmp.join("extra-a");
    let extra_b = tmp.join("extra-b");
    write(&extra_a.join("AGENTS.md"), "extra A agents");
    write(&extra_b.join("AGENTS.md"), "extra B agents");
    write(&extra_a.join("copilot-instructions.md"), "should be ignored");
    let raw = format!("{}, {}", extra_a.display(), extra_b.display());
    let parsed = HostDirs::ara(&tmp).with_env(|k| (k == "COPILOT_CUSTOM_INSTRUCTIONS_DIRS").then(|| raw.clone()));
    let (discovery, cwd, _) = copilot(&tmp, parsed.copilot_custom_instruction_dirs);
    let result = discovery.load_context_files(&cwd, &github_only());
    let contents: Vec<_> =
        result.all.iter().map(|(f, _)| f).filter(|f| f.level == Level::User).map(|f| f.content.as_str()).collect();
    assert!(contents.contains(&"extra A agents") && contents.contains(&"extra B agents"));
    assert!(!contents.contains(&"should be ignored"));
}

// --- fs-special-files.test.ts --------------------------------------------------

/// B-16043be376, B-a1688c97d4
#[test]
fn special_files_read_as_none_and_symlinks_are_followed() {
    let (_t, dir) = canonical_tempdir();
    let fifo = dir.join("CLAUDE.md");
    let made = std::process::Command::new("mkfifo").arg(&fifo).status().unwrap();
    assert!(made.success());
    let fs_cache = FsCache::new();
    let (tx, rx) = std::sync::mpsc::channel();
    let fifo_clone = fifo.clone();
    std::thread::spawn(move || {
        let _ = tx.send(FsCache::new().read_file(&fifo_clone));
    });
    let result = rx.recv_timeout(std::time::Duration::from_millis(1500));
    if result.is_err() {
        // Unblock a regressed reader before failing.
        let _ = fs::OpenOptions::new().write(true).open(&fifo);
    }
    assert_eq!(result.expect("read_file blocked on a FIFO"), None);

    let target = dir.join("AGENTS.md");
    write(&target, "# context");
    let link = dir.join("CLAUDE-link.md");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    assert_eq!(fs_cache.read_file(&link).as_deref(), Some("# context"));
}

// --- at-imports.test.ts --------------------------------------------------------

fn expand(fs_cache: &FsCache, content: &str, file: &Path, home: &Path) -> String {
    Expander { fs: fs_cache, home: home.to_path_buf(), max_depth: MAX_AT_IMPORT_DEPTH }.expand(content, file)
}

/// B-b01f6dfca7, B-99fbe51c1f, B-9af351552f, B-901394b1ee
#[test]
fn at_imports_inline_relative_home_and_nested() {
    let (_t, tmp) = canonical_tempdir();
    let fs_cache = FsCache::new();
    write(&tmp.join("AGENTS.md"), "ALWAYS use uppercase letters.");
    write(&tmp.join("CLAUDE.md"), "@AGENTS.md\n");
    let claude = tmp.join("CLAUDE.md");
    assert_eq!(expand(&fs_cache, "@AGENTS.md\n", &claude, &tmp).trim(), "ALWAYS use uppercase letters.");

    write(&tmp.join("rules/no-push.md"), "NEVER push.");
    let rules = tmp.join("rules/AGENTS.md");
    let out = expand(&fs_cache, "Rule: @./no-push.md\n", &rules, &tmp);
    assert!(out.contains("Rule: NEVER push.") && !out.contains("@./no-push.md"));

    let (_h, fake_home) = canonical_tempdir();
    write(&fake_home.join("prefs.md"), "use 2 spaces");
    assert!(expand(&fs_cache, "See @~/prefs.md.\n", &tmp.join("AGENTS.md"), &fake_home).contains("See use 2 spaces"));

    write(&tmp.join("c.md"), "LEAF.");
    write(&tmp.join("b.md"), "B then @c.md\n");
    write(&tmp.join("a.md"), "A then @b.md\n");
    assert!(expand(&fs_cache, "A then @b.md\n", &tmp.join("a.md"), &tmp).contains("A then B then LEAF."));
}

/// B-ca1ec4934f, B-8999628661, B-ccc51fcbf8
#[test]
fn at_imports_depth_cap_cycles_and_missing_files() {
    let (_t, tmp) = canonical_tempdir();
    let fs_cache = FsCache::new();
    let total = MAX_AT_IMPORT_DEPTH + 2;
    for i in 0..total {
        let body = if i == total - 1 { "TERMINAL".to_string() } else { format!("step-{i} -> @step{}.md", i + 1) };
        write(&tmp.join(format!("step{i}.md")), &format!("{body}\n"));
    }
    let entry = tmp.join("step0.md");
    let out = expand(&fs_cache, &fs::read_to_string(&entry).unwrap(), &entry, &tmp);
    assert!(out.contains(&format!("@step{}.md", MAX_AT_IMPORT_DEPTH + 1)) && !out.contains("TERMINAL"));

    write(&tmp.join("loop-a.md"), "A: @loop-b.md\n");
    write(&tmp.join("loop-b.md"), "B: @loop-a.md\n");
    let a = tmp.join("loop-a.md");
    let out = expand(&fs_cache, "A: @loop-b.md\n", &a, &tmp);
    assert!(out.contains("A: B:") && out.contains("@loop-a.md"));

    assert!(
        expand(&fs_cache, "See @./does-not-exist.md\n", &tmp.join("AGENTS.md"), &tmp).contains("@./does-not-exist.md")
    );
}

/// B-bdf993650b, B-8c42a1a35c, B-5527b79dfc, B-277d15f86b
#[test]
fn at_imports_skip_code_emails_and_strip_punctuation() {
    let (_t, tmp) = canonical_tempdir();
    let fs_cache = FsCache::new();
    write(&tmp.join("guide.md"), "INLINED");
    let source = tmp.join("AGENTS.md");
    let fenced = ["Run this:", "```bash", "echo @./guide.md", "```", "Also see @./guide.md."].join("\n");
    let out = expand(&fs_cache, &fenced, &source, &tmp);
    assert!(out.contains("echo @./guide.md") && out.contains("Also see INLINED"));

    let out = expand(&fs_cache, "Install via `npm i @./guide.md` and also @./guide.md.\n", &source, &tmp);
    assert!(out.contains("`npm i @./guide.md`") && out.contains("also INLINED"));

    let emails = "Ping me at me@example.com or use git@github.com:foo/bar.git for clones.\n";
    assert_eq!(expand(&fs_cache, emails, &source, &tmp), emails);

    assert!(
        expand(&fs_cache, "See @./guide.md, and then continue.\n", &source, &tmp)
            .contains("See INLINED, and then continue.")
    );
}

// --- system-prompt-context-dedup.test.ts -----------------------------------------

fn file(path: &str, content: &str, depth: Option<i64>) -> ProjectContextFile {
    ProjectContextFile {
        path: PathBuf::from(path),
        content: content.into(),
        depth,
        source: SourceMeta::new("test", Path::new(path), Level::Project),
    }
}

fn paths(files: Vec<ProjectContextFile>) -> Vec<String> {
    files.into_iter().map(|f| f.path.display().to_string()).collect()
}

/// B-b8d766802d, B-2622195252, B-a3bb895724, B-10d349c36a, B-8ff336dbb8
#[test]
fn dedupe_containment_rules() {
    let content = "Rule one.\n\nRule two.\n\nRule three.";
    let files =
        vec![file("/home/user/.config/AGENTS.md", content, Some(5)), file("/project/AGENTS.md", content, Some(0))];
    assert_eq!(paths(dedupe_contained_context_files(files)), ["/project/AGENTS.md"]);

    let files = vec![
        file("/home/user/.config/AGENTS.md", "Shared rule A.\n\nShared rule B.\n\nShared rule C.", Some(5)),
        file(
            "/project/AGENTS.md",
            "Shared rule A.\n\nShared rule B.\n\nShared rule C.\n\nProject-specific rule.",
            Some(0),
        ),
    ];
    assert_eq!(paths(dedupe_contained_context_files(files)), ["/project/AGENTS.md"]);

    let files = vec![
        file("/home/user/.config/AGENTS.md", "First.\n\nSecond.\n\nThird.", Some(5)),
        file("/project/AGENTS.md", "First.\n\nInterleaved.\n\nSecond.\n\nThird.", Some(0)),
    ];
    assert_eq!(paths(dedupe_contained_context_files(files)), ["/home/user/.config/AGENTS.md", "/project/AGENTS.md"]);

    let files = vec![
        file("/home/user/.config/AGENTS.md", "Always use tabs.\n\nNever commit directly.", Some(5)),
        file("/project/AGENTS.md", "Always use spaces.\n\nNever commit directly to main.", Some(0)),
    ];
    assert_eq!(paths(dedupe_contained_context_files(files)), ["/home/user/.config/AGENTS.md", "/project/AGENTS.md"]);

    let files = vec![
        file("/a/AGENTS.md", "Alpha rules.\n\nBeta rules.", Some(3)),
        file("/b/AGENTS.md", "Gamma rules.\n\nDelta rules.", Some(2)),
        file("/c/AGENTS.md", "Epsilon rules.\n\nZeta rules.", Some(0)),
    ];
    assert_eq!(paths(dedupe_contained_context_files(files)), ["/a/AGENTS.md", "/b/AGENTS.md", "/c/AGENTS.md"]);
}

/// B-bd1f7c0a74, B-dc6f5c4902, B-337aac551d, B-501506cdde, B-a78092b610, B-fed4a82641
#[test]
fn dedupe_authority_normalization_and_fences() {
    let files = vec![file("/empty/AGENTS.md", "", Some(5)), file("/project/AGENTS.md", "Real content.", Some(0))];
    assert_eq!(paths(dedupe_contained_context_files(files)), ["/empty/AGENTS.md", "/project/AGENTS.md"]);

    let files = vec![
        file("/level0/AGENTS.md", "Rule one.\n\nRule two.", Some(10)),
        file("/level1/AGENTS.md", "Rule one.\n\nRule two.\n\nRule three.", Some(5)),
        file("/level2/AGENTS.md", "Rule one.\n\nRule two.\n\nRule three.\n\nRule four.", Some(0)),
    ];
    assert_eq!(paths(dedupe_contained_context_files(files)), ["/level2/AGENTS.md"]);

    let files = vec![
        file("/home/user/.config/AGENTS.md", "  Rule one.  \n\n  Rule two.  ", Some(5)),
        file("/project/AGENTS.md", "Rule one.\n\nRule two.\n\nRule three.", Some(0)),
    ];
    assert_eq!(paths(dedupe_contained_context_files(files)), ["/project/AGENTS.md"]);

    let files = vec![
        file("/project/AGENTS.md", "Shared rule.", Some(0)),
        file("/home/user/.config/AGENTS.md", "Shared rule.\n\nFar-only rule.", Some(5)),
    ];
    assert_eq!(paths(dedupe_contained_context_files(files)), ["/home/user/.config/AGENTS.md", "/project/AGENTS.md"]);

    let files = vec![
        file("/project/AGENTS.md", "Shared rule.", Some(0)),
        file("/home/user/.omp/AGENTS.md", "Shared rule.\n\nUser-only rule.", None),
    ];
    assert_eq!(paths(dedupe_contained_context_files(files)), ["/home/user/.omp/AGENTS.md", "/project/AGENTS.md"]);

    let files = vec![
        file("/home/user/.config/AGENTS.md", "Never delete user data.", Some(5)),
        file("/project/AGENTS.md", "Example of a bad prompt:\n\n```\nNever delete user data.\n```", Some(0)),
    ];
    assert_eq!(paths(dedupe_contained_context_files(files)), ["/home/user/.config/AGENTS.md", "/project/AGENTS.md"]);
}
