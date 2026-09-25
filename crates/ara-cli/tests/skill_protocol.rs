//! Ports of OMP `packages/coding-agent/test/skill-protocol-customdirs.test.ts`
//! at 596f2da7101178214aa27a753529d15e6b7ad91d: skills loaded by
//! `ara-discovery` and handed to `ara-tools` the way the CLI host does,
//! then read through `skill://`.
//!
//! Not ported: B-3deff3fcd6 (semicolon-delimited multi-path reads) and
//! B-20f58d354f (tail with a leading context line); both depend on read
//! features that are open in TOOLS-01.

use ara_agent::{AgentTool, UpdateFn};
use ara_ai::UserBlock;
use ara_discovery::skills::SkillsSettings;
use ara_discovery::{Discovery, HostDirs, LoadedSkill, ProviderPolicy};
use ara_tools::internal_urls::SkillRef;
use ara_tools::{ToolContext, read::ReadTool};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

fn make_skill_md(name: &str, from: &str) -> String {
    format!("---\nname: {name}\ndescription: {name} skill.\n---\n\n# {name} from {from}\n")
}

fn write(dir: &Path, name: &str, from: &str) -> PathBuf {
    let skill_dir = dir.join(name);
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(skill_dir.join("SKILL.md"), make_skill_md(name, from)).unwrap();
    skill_dir
}

fn all_default_sources_disabled() -> SkillsSettings {
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

/// The CLI host's hand-off from discovery to the tools.
fn refs(skills: &[LoadedSkill]) -> Vec<SkillRef> {
    skills
        .iter()
        .map(|s| SkillRef { name: s.name.clone(), file_path: s.file_path.clone(), base_dir: s.base_dir.clone() })
        .collect()
}

async fn read(cwd: &Path, skills: &[LoadedSkill], url: &str) -> (String, String) {
    let tool = ReadTool { ctx: ToolContext::new(cwd).with_skills(refs(skills)) };
    let noop: UpdateFn = Arc::new(|_| {});
    let args = json!({ "path": url }).as_object().unwrap().clone();
    let out = tool.execute("c", args, CancellationToken::new(), noop).await.unwrap();
    let text = out
        .content
        .iter()
        .filter_map(|b| if let UserBlock::Text(t) = b { Some(t.text.clone()) } else { None })
        .collect::<Vec<_>>()
        .join("\n");
    (out.details.unwrap()["resolvedPath"].as_str().unwrap().to_string(), text)
}

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

fn env() -> Env {
    let dir = tempfile::Builder::new().prefix("ara-skill-protocol-").tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    fs::create_dir_all(root.join("home")).unwrap();
    Env { _dir: dir, root }
}

fn discovery(env: &Env) -> Discovery {
    let home = env.root.join("home");
    Discovery::new(&home, HostDirs::ara(&home), ProviderPolicy::default())
}

/// B-edc3d29fb5
#[tokio::test]
async fn resolves_a_skill_loaded_from_a_custom_directory() {
    let env = env();
    let custom = env.root.join("custom");
    let skill_dir = write(&custom, "my-custom-skill", &custom.display().to_string());
    let settings =
        SkillsSettings { custom_directories: vec![custom.display().to_string()], ..all_default_sources_disabled() };
    let (skills, _) = discovery(&env).load_skills(&env.root, &settings);
    let (resolved, text) = read(&env.root, &skills, "skill://my-custom-skill/").await;
    assert_eq!(resolved, skill_dir.join("SKILL.md").display().to_string());
    assert!(text.contains(&format!("from {}", custom.display())), "{text}");
}

/// B-4ebceb9b09
#[tokio::test]
async fn keeps_first_wins_across_multiple_custom_directories() {
    let env = env();
    let (dir_a, dir_b) = (env.root.join("custom-a"), env.root.join("custom-b"));
    let skill_a = write(&dir_a, "same-name", "a");
    write(&dir_b, "same-name", "b");
    let settings = SkillsSettings {
        custom_directories: vec![dir_a.display().to_string(), dir_b.display().to_string()],
        ..all_default_sources_disabled()
    };
    let (skills, warnings) = discovery(&env).load_skills(&env.root, &settings);
    let dup: Vec<_> = skills.iter().filter(|s| s.name == "same-name").collect();
    assert_eq!(dup.len(), 1);
    assert_eq!(dup[0].file_path, skill_a.join("SKILL.md"));
    assert!(warnings.iter().any(|w| w.message.contains("collision")), "{warnings:?}");
    let (resolved, _) = read(&env.root, &skills, "skill://same-name/").await;
    assert_eq!(resolved, skill_a.join("SKILL.md").display().to_string());
}

/// B-0b170f55e3
#[tokio::test]
async fn custom_directory_overrides_a_same_named_default_path_skill() {
    let env = env();
    let cwd = env.root.join("project");
    write(&cwd.join(".claude/skills"), "shared-name", "default");
    let custom = env.root.join("custom");
    let custom_skill = write(&custom, "shared-name", "custom");
    let settings = SkillsSettings {
        enable_claude_project: true,
        custom_directories: vec![custom.display().to_string()],
        ..all_default_sources_disabled()
    };
    let (skills, _) = discovery(&env).load_skills(&cwd, &settings);
    let dup = skills.iter().find(|s| s.name == "shared-name").unwrap();
    assert_eq!(dup.file_path, custom_skill.join("SKILL.md"));
    let (resolved, text) = read(&cwd, &skills, "skill://shared-name/").await;
    assert_eq!(resolved, custom_skill.join("SKILL.md").display().to_string());
    assert!(text.contains("from custom"), "{text}");
}
