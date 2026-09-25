//! Port of OMP `packages/coding-agent/test/skills.test.ts` (loading cases) at
//! 596f2da7101178214aa27a753529d15e6b7ad91d. `tests/fixtures/skills*` are
//! copied from upstream `test/fixtures/skills*` (MIT, Stencil Labs).
//!
//! Intentional difference: B-394494e571, B-9ca7e04896 and B-82d9ed5857 test
//! upstream's `cmd.exe`/`wslpath` host probes; ARA spawns no process during
//! discovery and takes the WSL home from `HostDirs::with_env`.

use ara_discovery::skills::{SkillsSettings, load_skills_from_dir};
use ara_discovery::{Discovery, FsCache, HostDirs, Level, ProviderPolicy};
use std::fs;
use std::path::{Path, PathBuf};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/skills")
}

const LONG_NAME: &str =
    "this-is-a-very-long-skill-name-that-exceeds-the-sixty-four-character-limit-set-by-the-standard";
const FIXTURE_ORDER: [&str; 6] =
    ["bad--name", "different-name", "Invalid_Name", LONG_NAME, "unknown-field", "valid-skill"];

fn all_builtins_off() -> SkillsSettings {
    SkillsSettings {
        enable_codex_user: false,
        enable_claude_user: false,
        enable_claude_project: false,
        enable_native_user: false,
        enable_native_project: false,
        enable_agents_user: false,
        enable_agents_project: false,
        ..SkillsSettings::default()
    }
}

fn write_skill(dir: &Path, name: &str, body: &str) {
    fs::create_dir_all(dir.join(name)).unwrap();
    fs::write(dir.join(name).join("SKILL.md"), body).unwrap();
}

struct Env {
    _dir: tempfile::TempDir,
    home: PathBuf,
    cwd: PathBuf,
}

fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let (home, cwd) = (root.join("home"), root.join("cwd"));
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&cwd).unwrap();
    Env { _dir: dir, home, cwd }
}

fn discovery(env: &Env) -> Discovery {
    Discovery::new(&env.home, HostDirs::ara(&env.home), ProviderPolicy::default())
}

/// B-9309699796, B-96aa491917, B-62d4fee77e, B-f38717b563, B-8e61346d8e,
/// B-3e71d641ca, B-6a437c1930, B-f449f1d56a, B-f5c0ba1e40, B-35d4eea288,
/// B-e9a28ee4c2, B-797be2cac9, B-c4038b5269
#[test]
fn load_skills_from_dir_fixture_root() {
    let fs_cache = FsCache::new();
    let (skills, warnings) = load_skills_from_dir(&fs_cache, &fixtures(), "test");
    let valid = skills.iter().find(|s| s.name == "valid-skill").unwrap();
    assert_eq!(valid.description, "A valid skill for testing purposes.");
    assert_eq!(valid.source, "test");
    assert!(warnings.is_empty());
    let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, FIXTURE_ORDER);
    assert!(
        !names.contains(&"missing-description")
            && !names.contains(&"child-skill")
            && !names.contains(&"no-frontmatter")
    );

    let (missing, warnings) = load_skills_from_dir(&fs_cache, Path::new("/non/existent/path"), "test");
    assert!(missing.is_empty() && warnings.is_empty());
    let (single, _) = load_skills_from_dir(&fs_cache, &fixtures().join("valid-skill"), "test");
    assert!(single.is_empty());
}

/// B-3f655f15ec, B-c5a3e48ff5, B-a7e59a2703
#[test]
fn custom_directories_only_when_builtins_disabled() {
    let env = env();
    let d = discovery(&env);
    let settings = SkillsSettings { custom_directories: vec![fixtures().display().to_string()], ..all_builtins_off() };
    let (skills, _) = d.load_skills(&env.cwd, &settings);
    assert!(skills.iter().all(|s| s.source.starts_with("custom")));
    assert_eq!(skills.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), FIXTURE_ORDER);
    assert_eq!(skills[0].meta.provider_name, "Custom");
    assert!(d.load_skills(&env.cwd, &all_builtins_off()).0.is_empty());
}

/// B-3b591df128
#[test]
fn claude_user_skills_load_without_project_dir() {
    let env = env();
    write_skill(
        &env.home.join(".claude/skills"),
        "user-only-skill",
        "---\nname: user-only-skill\ndescription: User-only Claude skill\n---\n\n# User-only skill",
    );
    let mut policy = ProviderPolicy::default();
    policy.enabled_user_sources.insert("claude".into());
    let d = Discovery::new(&env.home, HostDirs::ara(&env.home), policy);
    let ctx = d.context(&env.cwd);
    let claude = d.skills.providers().iter().find(|p| p.id == "claude").unwrap();
    let result = (claude.load)(&ctx).unwrap();
    assert!(result.items.iter().any(|s| s.name == "user-only-skill" && s.level == Level::User));
}

/// B-9f45d8c0d2, B-6e60ed9e9f, B-b1ce16038d
#[test]
fn agents_toggles_and_third_party_gate() {
    let env = env();
    write_skill(
        &env.home.join(".agents/skills"),
        "user-agents-skill",
        "---\ndescription: Loaded from ~/.agents/skills\n---\n\n# user-agents-skill",
    );
    write_skill(
        &env.home.join(".config/opencode/skills"),
        "leaked-opencode",
        "---\ndescription: Should be filtered by third-party gate\n---\n",
    );
    let d = discovery(&env);
    let partial = SkillsSettings { enable_agents_user: true, enable_agents_project: true, ..all_builtins_off() };
    let (skills, _) = d.load_skills(&env.cwd, &partial);
    assert!(skills.iter().any(|s| s.name == "user-agents-skill" && s.source == "agents:user"));
    assert!(!skills.iter().any(|s| s.name == "leaked-opencode"));
    let (off, _) = d.load_skills(&env.cwd, &all_builtins_off());
    assert!(!off.iter().any(|s| s.name == "user-agents-skill"));
}

/// B-7e1875d07a, B-42b0f9a421
#[test]
fn wsl_host_agents_skills() {
    let env = env();
    let host_home = env.home.parent().unwrap().join("host");
    write_skill(
        &host_home.join(".agents/skills"),
        "wsl-host-skill",
        "---\ndescription: Loaded from WSL host USERPROFILE\n---\n\n# wsl-host-skill",
    );
    let host = host_home.display().to_string();
    let dirs = HostDirs::ara(&env.home).with_env(|k| match k {
        "WSL_DISTRO_NAME" => Some("Ubuntu".into()),
        "USERPROFILE" => Some(host.clone()),
        _ => None,
    });
    assert_eq!(dirs.extra_user_homes, std::slice::from_ref(&host_home));
    let converted = HostDirs::ara(&env.home).with_env(|k| match k {
        "WSL_INTEROP" => Some("/run/WSL/1_interop".into()),
        "USERPROFILE" => Some(r"C:\Users\me".into()),
        _ => None,
    });
    assert_eq!(converted.extra_user_homes, [PathBuf::from("/mnt/c/Users/me")]);
    let d = Discovery::new(&env.home, dirs, ProviderPolicy::default());
    let settings = SkillsSettings { enable_agents_user: true, enable_agents_project: true, ..all_builtins_off() };
    let (skills, _) = d.load_skills(&env.cwd, &settings);
    let skill = skills.iter().find(|s| s.name == "wsl-host-skill").unwrap();
    assert_eq!(skill.source, "agents:user");
    assert_eq!(skill.file_path, host_home.join(".agents/skills/wsl-host-skill/SKILL.md"));
}

/// B-f60db39121, B-f68a925db4, B-1987d7ed8c, B-1c338d8224, B-f33a39c016, B-4fd4e4c850
#[test]
fn include_and_ignore_globs() {
    let env = env();
    let d = discovery(&env);
    let with = |include: &[&str], ignore: &[&str]| {
        let settings = SkillsSettings {
            custom_directories: vec![fixtures().display().to_string()],
            include_skills: include.iter().map(|s| s.to_string()).collect(),
            ignored_skills: ignore.iter().map(|s| s.to_string()).collect(),
            ..all_builtins_off()
        };
        d.load_skills(&env.cwd, &settings).0.into_iter().map(|s| s.name).collect::<Vec<_>>()
    };
    assert!(!with(&[], &["valid-skill"]).contains(&"valid-skill".to_string()));
    assert!(with(&[], &["valid-*"]).iter().all(|n| !n.starts_with("valid-")));
    assert!(with(&["valid-*"], &["valid-skill"]).iter().all(|n| n != "valid-skill"));
    assert_eq!(with(&["valid-skill"], &[]), ["valid-skill"]);
    let valid_only = with(&["valid-*"], &[]);
    assert!(!valid_only.is_empty() && valid_only.iter().all(|n| n.starts_with("valid-")));
    assert_eq!(with(&[], &[]).len(), FIXTURE_ORDER.len());
}

/// B-0001679451, B-5a17228bb2
#[test]
fn frontmatter_enabled_false_and_disable_model_invocation() {
    let env = env();
    let dir = env.home.join("custom");
    write_skill(
        &dir,
        "disabled-skill",
        "---\nname: disabled-skill\ndescription: Should not be discovered.\nenabled: false\n---\n\n# Disabled Skill\n",
    );
    write_skill(
        &dir,
        "hidden-by-spec",
        "---\nname: hidden-by-spec\ndescription: Should be hidden via Agent Skills standard field.\ndisable-model-invocation: true\n---\n\n# Hidden Skill\n",
    );
    let d = discovery(&env);
    let (skills, _) = d.load_skills(
        &env.cwd,
        &SkillsSettings { custom_directories: vec![dir.display().to_string()], ..all_builtins_off() },
    );
    assert!(!skills.iter().any(|s| s.name == "disabled-skill"));
    assert!(skills.iter().find(|s| s.name == "hidden-by-spec").unwrap().hide);
}

/// B-ba73c5972b
#[test]
fn tilde_custom_directories() {
    let env = env();
    write_skill(
        &env.home.join(".pi-skills-test"),
        "tilde-skill",
        "---\nname: tilde-skill\ndescription: Skill loaded from a tilde-expanded custom directory.\n---\n\n# Tilde Skill\n",
    );
    let d = discovery(&env);
    let load = |dir: String| {
        d.load_skills(&env.cwd, &SkillsSettings { custom_directories: vec![dir], ..all_builtins_off() }).0
    };
    let with_tilde = load("~/.pi-skills-test".into());
    let without = load(env.home.join(".pi-skills-test").display().to_string());
    assert_eq!(with_tilde.len(), without.len());
    assert!(with_tilde.iter().any(|s| s.name == "tilde-skill"));
}

/// B-25678d10e2
#[test]
fn collision_fixtures_share_a_name() {
    let fs_cache = FsCache::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/skills-collision");
    let (first, _) = load_skills_from_dir(&fs_cache, &root.join("first"), "first");
    let (second, _) = load_skills_from_dir(&fs_cache, &root.join("second"), "second");
    assert_eq!((first.len(), second.len()), (1, 1));
    assert_eq!((first[0].name.as_str(), second[0].name.as_str()), ("calendar", "calendar"));

    // Through the loader: two custom directories, first wins with a warning.
    let env = env();
    let d = discovery(&env);
    let settings = SkillsSettings {
        custom_directories: vec![root.join("first").display().to_string(), root.join("second").display().to_string()],
        ..all_builtins_off()
    };
    let (skills, warnings) = d.load_skills(&env.cwd, &settings);
    assert_eq!(skills.len(), 1);
    assert!(skills[0].file_path.starts_with(root.join("first")));
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].message.starts_with("name collision: \"calendar\" already loaded from "));
}

/// ARA scenarios for `loadSkills` rules upstream documents in code comments.
#[test]
fn loader_precedence_symlinks_and_custom_override() {
    let env = env();
    let repo = env.home.join("repo");
    let cwd = repo.join("app");
    fs::create_dir_all(repo.join(".git")).unwrap();
    fs::create_dir_all(&cwd).unwrap();
    // Same name from native (priority 100, disabled below) and .agents (70).
    write_skill(&repo.join(".ara/skills"), "shared", "---\ndescription: native copy\n---\n");
    write_skill(&repo.join(".agents/skills"), "shared", "---\ndescription: agents copy\n---\n");
    // A symlinked duplicate of another skill directory.
    write_skill(&repo.join(".agents/skills"), "real", "---\ndescription: real one\n---\n");
    std::os::unix::fs::symlink(repo.join(".agents/skills/real"), repo.join(".claude/skills/alias").as_path())
        .unwrap_or_else(|_| {
            fs::create_dir_all(repo.join(".claude/skills")).unwrap();
            std::os::unix::fs::symlink(repo.join(".agents/skills/real"), repo.join(".claude/skills/alias")).unwrap();
        });
    let d = discovery(&env);
    let settings = SkillsSettings { enable_native_project: false, ..SkillsSettings::default() };
    let (skills, _) = d.load_skills(&cwd, &settings);
    let shared = skills.iter().find(|s| s.name == "shared").unwrap();
    assert_eq!((shared.source.as_str(), shared.description.as_str()), ("agents:project", "agents copy"));
    // `alias` (claude, priority 80) and `real` (agents) are the same file: one survives.
    let same_file: Vec<_> = skills.iter().filter(|s| s.description == "real one").collect();
    assert_eq!(same_file.len(), 1);

    // A custom directory replaces a default-provider skill of the same name.
    let custom = env.home.join("custom");
    write_skill(&custom, "shared", "---\ndescription: custom copy\n---\n");
    let settings = SkillsSettings { custom_directories: vec![custom.display().to_string()], ..settings };
    let (skills, _) = d.load_skills(&cwd, &settings);
    let shared = skills.iter().find(|s| s.name == "shared").unwrap();
    assert_eq!((shared.source.as_str(), shared.description.as_str()), ("custom:user", "custom copy"));
}
