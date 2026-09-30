//! Host-selected compatibility paths are Skill-only switches. The legacy
//! fixed-OMP settings remain available when no switches are supplied.

use ara_discovery::{
    Discovery, HostDirs, Level, LoadResult, Provider, ProviderPolicy, SkillSourceSwitches, SkillsSettings,
};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const SOURCES: [(&str, &str, &str); 4] = [
    ("agents", ".agents/skills", ".agents/skills"),
    ("claude", ".claude/skills", ".claude/skills"),
    ("codex", ".codex/skills", ".codex/skills"),
    ("opencode", ".config/opencode/skills", ".opencode/skills"),
];

struct Env {
    _root: tempfile::TempDir,
    home: PathBuf,
    cwd: PathBuf,
}

impl Env {
    fn new() -> Self {
        let root = tempfile::Builder::new().prefix("ara-skill-sources-").tempdir().unwrap();
        let home = root.path().join("home");
        let cwd = root.path().join("repo");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(cwd.join(".git")).unwrap();
        Self { _root: root, home, cwd }
    }

    fn discovery(&self, policy: ProviderPolicy) -> Discovery {
        Discovery::new(&self.home, HostDirs::ara(&self.home), policy)
    }

    fn fixtures(&self) {
        for (source, user, project) in SOURCES {
            write_skill(&self.home.join(user), &format!("{source}-user"), source);
            write_skill(&self.cwd.join(project), &format!("{source}-project"), source);
        }
    }
}

fn write_skill(directory: &Path, name: &str, description: &str) -> PathBuf {
    let path = directory.join(name).join("SKILL.md");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, format!("---\ndescription: {description}\n---\nBody of {name}.\n")).unwrap();
    path
}

fn switches(bits: u8) -> SkillSourceSwitches {
    SkillSourceSwitches { agents: bits & 1 != 0, claude: bits & 2 != 0, codex: bits & 4 != 0, opencode: bits & 8 != 0 }
}

fn settings(sources: SkillSourceSwitches) -> SkillsSettings {
    SkillsSettings { source_switches: Some(sources), ..SkillsSettings::default() }
}

#[test]
fn default_switches_partial_json_and_roundtrip_match_the_selected_defaults() {
    let expected = switches(7);
    assert_eq!(SkillSourceSwitches::default(), expected);
    assert_eq!(serde_json::from_value::<SkillSourceSwitches>(json!({})).unwrap(), expected);
    assert_eq!(
        serde_json::from_value::<SkillSourceSwitches>(json!({"claude": false, "opencode": true})).unwrap(),
        switches(13)
    );
    assert!(SkillsSettings::default().source_switches.is_none(), "legacy callers keep fixed-OMP defaults");
    for bits in 0..16 {
        let selected = switches(bits);
        let encoded = serde_json::to_value(selected).unwrap();
        assert_eq!(
            encoded,
            json!({
                "agents": selected.agents, "claude": selected.claude,
                "codex": selected.codex, "opencode": selected.opencode
            })
        );
        assert_eq!(serde_json::from_value::<SkillSourceSwitches>(encoded).unwrap(), selected);
    }
    for invalid in [json!({"claude": "false"}), json!({"agents": null}), json!({"unknown": true})] {
        assert!(serde_json::from_value::<SkillSourceSwitches>(invalid).is_err());
    }
}

#[test]
fn csv_selection_is_an_exact_allowlist_including_an_empty_selection() {
    for (csv, expected) in [
        ("agents,claude,codex", switches(7)),
        (" opencode , codex , opencode , ", switches(12)),
        ("", switches(0)),
        (" , , ", switches(0)),
    ] {
        assert_eq!(csv.parse::<SkillSourceSwitches>().unwrap(), expected, "{csv:?}");
    }
    for csv in ["unknown", "agents,unknown", "Agents", "*", "all"] {
        assert!(csv.parse::<SkillSourceSwitches>().is_err(), "{csv:?}");
    }
}

#[test]
fn every_switch_combination_controls_both_user_and_project_paths() {
    let env = Env::new();
    env.fixtures();
    let discovery = env.discovery(ProviderPolicy::default());
    for bits in 0..16 {
        let selected = switches(bits);
        let settings = SkillsSettings {
            // The compatibility switches own these paths even when legacy
            // booleans request the opposite value.
            enable_agents_user: !selected.agents,
            enable_agents_project: !selected.agents,
            enable_claude_user: !selected.claude,
            enable_claude_project: !selected.claude,
            enable_codex_user: !selected.codex,
            ..settings(selected)
        };
        let (skills, warnings) = discovery.load_skills(&env.cwd, &settings);
        assert!(warnings.is_empty(), "{bits}: {warnings:?}");
        assert_eq!(skills.len(), 2 * bits.count_ones() as usize, "{bits}: {skills:?}");
        for (index, (source, user, project)) in SOURCES.iter().enumerate() {
            for (suffix, directory, level) in
                [("user", env.home.join(user), Level::User), ("project", env.cwd.join(project), Level::Project)]
            {
                let name = format!("{source}-{suffix}");
                let found = skills.iter().find(|skill| skill.name == name);
                assert_eq!(found.is_some(), bits & (1 << index) != 0, "{bits}: {name}");
                if let Some(skill) = found {
                    assert_eq!(skill.source, format!("{source}:{suffix}"));
                    assert_eq!(skill.meta.level, level);
                    assert_eq!(skill.file_path, directory.join(name).join("SKILL.md"));
                }
            }
        }
    }
}

#[test]
fn off_sources_are_not_scanned_despite_wildcards_overrides_and_legacy_optins() {
    let env = Env::new();
    env.fixtures();
    let override_dir = env.home.join("claude-override");
    write_skill(&override_dir.join("skills"), "claude-override", "override");
    for wildcard in ["*", "all"] {
        let mut policy = ProviderPolicy::default();
        policy.enabled_user_sources.insert(wildcard.into());
        let mut discovery = env.discovery(policy);
        discovery.dirs.claude_config_dir = Some(override_dir.clone());
        let counters: Vec<_> = SOURCES
            .iter()
            .map(|(source, _, _)| {
                let calls = Arc::new(AtomicUsize::new(0));
                let observed = Arc::clone(&calls);
                discovery.skills.register(Provider {
                    id: (*source).into(),
                    display_name: format!("{source} scan witness"),
                    description: "Must not execute when the source is off".into(),
                    priority: 200,
                    load: Arc::new(move |_| {
                        observed.fetch_add(1, Ordering::SeqCst);
                        Ok(LoadResult::default())
                    }),
                });
                calls
            })
            .collect();
        let all_off = SkillsSettings { enable_claude_user: true, enable_codex_user: true, ..settings(switches(0)) };
        let (skills, warnings) = discovery.load_skills(&env.cwd, &all_off);
        assert!(skills.is_empty() && warnings.is_empty(), "{wildcard}: {skills:?}, {warnings:?}");
        assert!(counters.iter().all(|calls| calls.load(Ordering::SeqCst) == 0));
        let (on, _) = discovery.load_skills(&env.cwd, &settings(switches(15)));
        assert_eq!(on.len(), 8);
        assert!(on.iter().any(|skill| skill.name == "claude-override"));
        assert!(counters.iter().all(|calls| calls.load(Ordering::SeqCst) == 1));
    }
}

#[test]
fn general_disabled_policy_vetoes_enabled_skill_sources_before_loading() {
    let env = Env::new();
    env.fixtures();
    for (source, _, _) in SOURCES {
        let mut policy = ProviderPolicy::default();
        policy.disabled.insert(source.into());
        policy.enabled_user_sources.insert("all".into());
        let mut discovery = env.discovery(policy);
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&calls);
        discovery.skills.register(Provider {
            id: source.into(),
            display_name: "Disabled provider witness".into(),
            description: "General policy must veto Skill opt-in".into(),
            priority: 200,
            load: Arc::new(move |_| {
                observed.fetch_add(1, Ordering::SeqCst);
                Ok(LoadResult::default())
            }),
        });
        let (skills, warnings) = discovery.load_skills(&env.cwd, &settings(switches(15)));
        assert_eq!(skills.len(), 6, "{source}: {skills:?}");
        assert!(skills.iter().all(|skill| skill.meta.provider != source));
        assert!(warnings.is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn disabling_the_higher_priority_source_exposes_the_enabled_same_name_skill() {
    let env = Env::new();
    write_skill(&env.cwd.join(".claude/skills"), "shared", "Claude copy");
    let agents = write_skill(&env.cwd.join(".agents/skills"), "shared", "Agents copy");
    let discovery = env.discovery(ProviderPolicy::default());
    let (both, _) = discovery.load_skills(&env.cwd, &settings(switches(3)));
    assert_eq!(both.len(), 1);
    assert_eq!(both[0].description, "Claude copy");
    let (fallback, warnings) = discovery.load_skills(&env.cwd, &settings(switches(1)));
    assert_eq!(fallback.len(), 1);
    assert_eq!(fallback[0].description, "Agents copy");
    assert_eq!(fallback[0].file_path, agents);
    assert!(warnings.is_empty());
}

#[test]
fn native_github_custom_and_context_discovery_keep_their_existing_controls() {
    let env = Env::new();
    env.fixtures();
    let custom = env.home.join("custom-skills");
    write_skill(&env.home.join(".ara/agent/skills"), "native-user", "Native user");
    write_skill(&env.cwd.join(".ara/skills"), "native-project", "Native project");
    write_skill(&env.cwd.join(".github/skills"), "github-project", "GitHub");
    write_skill(&custom, "custom", "Custom");
    fs::write(env.cwd.join(".claude/CLAUDE.md"), "Project context remains visible.").unwrap();
    fs::write(env.home.join(".claude/CLAUDE.md"), "User context stays opted out.").unwrap();
    let discovery = env.discovery(ProviderPolicy::default());
    let before = discovery.load_project_context_files(&env.cwd, &[]);
    assert!(before.iter().any(|file| file.content.contains("Project context remains visible.")));
    assert!(before.iter().all(|file| !file.content.contains("User context stays opted out.")));
    for bits in [0, 15] {
        let settings =
            SkillsSettings { custom_directories: vec![custom.display().to_string()], ..settings(switches(bits)) };
        let (skills, _) = discovery.load_skills(&env.cwd, &settings);
        for name in ["native-user", "native-project", "github-project", "custom"] {
            assert!(skills.iter().any(|skill| skill.name == name), "{bits}: {name}");
        }
        assert_eq!(discovery.load_project_context_files(&env.cwd, &[]), before);
        assert!(discovery.policy.enabled_user_sources.is_empty());
    }
    let disabled = SkillsSettings { enable_native_user: false, enable_native_project: false, ..settings(switches(0)) };
    assert!(discovery.load_skills(&env.cwd, &disabled).0.iter().all(|skill| skill.meta.provider != "native"));
    let total_off = SkillsSettings { enabled: false, ..settings(switches(15)) };
    assert!(discovery.load_skills(&env.cwd, &total_off).0.is_empty());
}
