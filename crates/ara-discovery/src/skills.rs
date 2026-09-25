//! Skills: `SKILL.md` discovery (`capability/skill.ts`, `discovery/helpers.ts`
//! `scanSkillsFromDir`, the `loadSkills` providers of `builtin`, `claude`,
//! `agents`, `codex`, `opencode` and `github`) and the loader with its
//! settings (`extensibility/skills.ts` `loadSkills`).
//!
//! Not ported here: plugin skills (Claude/Agent/OMP plugins) and managed
//! (auto-learn) skills; they belong to the extensibility and memory work.

use crate::capability::{Capability, Level, LoadContext, LoadOptions, LoadResult, Provider, SourceMeta, Sourced};
use crate::frontmatter::{FrontmatterOptions, parse_frontmatter};
use crate::fs::FsCache;
use crate::project::Discovery;
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A discovered `SKILL.md` (capability item).
#[derive(Clone, Debug, PartialEq)]
pub struct Skill {
    /// Frontmatter `name` (trimmed) or the skill directory's name.
    pub name: String,
    pub path: PathBuf,
    /// Markdown body without frontmatter.
    pub content: String,
    pub frontmatter: Map<String, Value>,
    pub level: Level,
    pub source: SourceMeta,
}

impl Sourced for Skill {
    fn source(&self) -> &SourceMeta {
        &self.source
    }
    fn source_mut(&mut self) -> &mut SourceMeta {
        &mut self.source
    }
}

/// Stable prompt order: name case-insensitively, then name, then path
/// (`compareSkillOrder`, UTF-16 code unit comparison).
pub fn compare_skill_order(a_name: &str, a_path: &Path, b_name: &str, b_path: &Path) -> std::cmp::Ordering {
    let units = |s: &str| s.encode_utf16().collect::<Vec<_>>();
    units(&js_lower(a_name))
        .cmp(&units(&js_lower(b_name)))
        .then_with(|| units(a_name).cmp(&units(b_name)))
        .then_with(|| units(&a_path.to_string_lossy()).cmp(&units(&b_path.to_string_lossy())))
}

/// JS `toLowerCase` (full Unicode lowercase mapping).
fn js_lower(s: &str) -> String {
    s.to_lowercase()
}

/// `scanSkillsFromDir`: `<dir>/<name>/SKILL.md` for each non-hidden
/// directory or symlink entry; `enabled: false` skips; a missing
/// description skips when `require_description`.
pub fn scan_skills_from_dir(
    fs: &FsCache,
    dir: &Path,
    provider: &str,
    level: Level,
    require_description: bool,
) -> LoadResult<Skill> {
    let mut result = LoadResult::default();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return result,
        Err(error) => {
            result.warnings.push(format!("Failed to read skills directory: {} ({error})", dir.display()));
            return result;
        }
    };
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let Ok(kind) = entry.file_type() else { continue };
        if !kind.is_dir() && !kind.is_symlink() {
            continue;
        }
        let skill_path = dir.join(&name).join("SKILL.md");
        if !skill_path.exists() {
            continue;
        }
        let Some(content) = fs.read_file(&skill_path).filter(|c| !c.is_empty()) else { continue };
        let options = FrontmatterOptions { source: Some(skill_path.display().to_string()), ..Default::default() };
        let parsed = match parse_frontmatter(&content, &options) {
            Ok(parsed) => parsed,
            Err(_) => {
                result.warnings.push(format!("Failed to read skill file: {}", skill_path.display()));
                continue;
            }
        };
        let fm = parsed.frontmatter;
        if fm.get("enabled") == Some(&Value::Bool(false)) {
            continue;
        }
        if require_description && !fm.get("description").is_some_and(ara_prompt::js::truthy) {
            continue;
        }
        let dir_name = name.to_string_lossy().into_owned();
        let skill_name = match fm.get("name") {
            Some(Value::String(n)) if !ara_prompt::js::trim(n).is_empty() => ara_prompt::js::trim(n).to_string(),
            _ => dir_name,
        };
        result.items.push(Skill {
            name: skill_name,
            source: SourceMeta::new(provider, &skill_path, level),
            path: skill_path,
            content: parsed.body,
            frontmatter: fm,
            level,
        });
    }
    result.items.sort_by(|a, b| compare_skill_order(&a.name, &a.path, &b.name, &b.path));
    result
}

fn merge(results: impl IntoIterator<Item = LoadResult<Skill>>) -> LoadResult<Skill> {
    let mut merged = LoadResult::default();
    for result in results {
        merged.items.extend(result.items);
        merged.warnings.extend(result.warnings);
    }
    merged
}

/// Ancestors of cwd up to `stop` (inclusive) or the filesystem root.
fn ancestors_until(cwd: &Path, stop: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut current = cwd.to_path_buf();
    loop {
        out.push(current.clone());
        if current == stop {
            break;
        }
        let Some(parent) = current.parent() else { break };
        current = parent.to_path_buf();
    }
    out
}

fn native_skills(ctx: &LoadContext<'_>) -> Result<LoadResult<Skill>, String> {
    let stop = ctx.repo_root.clone().unwrap_or_else(|| ctx.home.clone());
    let project = ancestors_until(&ctx.cwd, &stop).into_iter().map(|dir| {
        scan_skills_from_dir(
            ctx.fs,
            &dir.join(&ctx.dirs.native_project_dir).join("skills"),
            "native",
            Level::Project,
            true,
        )
    });
    let user = scan_skills_from_dir(ctx.fs, &ctx.dirs.native_user_dir.join("skills"), "native", Level::User, true);
    Ok(merge(project.collect::<Vec<_>>().into_iter().chain([user])))
}

fn claude_skills(ctx: &LoadContext<'_>) -> Result<LoadResult<Skill>, String> {
    let user = (ctx.skill_toggles.claude_user || ctx.is_user_source_enabled("claude")).then(|| {
        let dir = ctx.dirs.claude_config_dir.clone().unwrap_or_else(|| ctx.home.join(".claude"));
        scan_skills_from_dir(ctx.fs, &dir.join("skills"), "claude", Level::User, false)
    });
    // Project: walk up, skipping home (that is the user source).
    let stop = ctx.repo_root.clone().unwrap_or_else(|| ctx.home.clone());
    let project: Vec<_> = ancestors_until(&ctx.cwd, &stop)
        .into_iter()
        .filter(|dir| *dir != ctx.home)
        .map(|dir| scan_skills_from_dir(ctx.fs, &dir.join(".claude").join("skills"), "claude", Level::Project, false))
        .collect();
    Ok(merge(user.into_iter().chain(project)))
}

fn agents_skills(ctx: &LoadContext<'_>) -> Result<LoadResult<Skill>, String> {
    let stop = ctx.repo_root.clone().unwrap_or_else(|| ctx.home.clone());
    let mut scans = Vec::new();
    for dir in ancestors_until(&ctx.cwd, &stop) {
        if dir == ctx.home {
            continue;
        }
        for base in [".agent", ".agents"] {
            scans.push(scan_skills_from_dir(ctx.fs, &dir.join(base).join("skills"), "agents", Level::Project, false));
        }
    }
    let mut homes = vec![ctx.home.clone()];
    for extra in &ctx.dirs.extra_user_homes {
        if !homes.contains(extra) {
            homes.push(extra.clone());
        }
    }
    for home in homes {
        for base in [".agent", ".agents"] {
            scans.push(scan_skills_from_dir(ctx.fs, &home.join(base).join("skills"), "agents", Level::User, false));
        }
    }
    Ok(merge(scans))
}

fn codex_skills(ctx: &LoadContext<'_>) -> Result<LoadResult<Skill>, String> {
    let user = (ctx.skill_toggles.codex_user || ctx.is_user_source_enabled("codex"))
        .then(|| scan_skills_from_dir(ctx.fs, &ctx.home.join(".codex/skills"), "codex", Level::User, false));
    let project = scan_skills_from_dir(ctx.fs, &ctx.cwd.join(".codex/skills"), "codex", Level::Project, false);
    Ok(merge(user.into_iter().chain([project])))
}

fn opencode_skills(ctx: &LoadContext<'_>) -> Result<LoadResult<Skill>, String> {
    let user = ctx.is_user_source_enabled("opencode").then(|| {
        scan_skills_from_dir(ctx.fs, &ctx.home.join(".config/opencode/skills"), "opencode", Level::User, false)
    });
    let project = scan_skills_from_dir(ctx.fs, &ctx.cwd.join(".opencode/skills"), "opencode", Level::Project, false);
    Ok(merge(user.into_iter().chain([project])))
}

fn github_skills(ctx: &LoadContext<'_>) -> Result<LoadResult<Skill>, String> {
    Ok(scan_skills_from_dir(ctx.fs, &ctx.cwd.join(".github/skills"), "github", Level::Project, true))
}

fn provider(
    id: &str,
    display_name: &str,
    priority: i32,
    load: fn(&LoadContext<'_>) -> Result<LoadResult<Skill>, String>,
) -> Provider<Skill> {
    Provider {
        id: id.into(),
        display_name: display_name.into(),
        description: format!("Skills from {display_name}"),
        priority,
        load: Arc::new(load),
    }
}

/// The skills capability with upstream's directory providers in upstream's
/// registration order (ties: `.agents` before Codex).
pub fn skill_capability(native_name: &str) -> Capability<Skill> {
    let mut capability = Capability::new(
        "skills",
        "Skills",
        "Specialized knowledge and workflow files that extend agent capabilities",
        |skill: &Skill| Some(skill.name.clone()),
    );
    capability.to_extension_id = Some(|skill: &Skill| Some(format!("skill:{}", skill.name)));
    capability.validate = Some(|skill: &Skill| {
        if skill.name.is_empty() {
            Some("Missing skill name".into())
        } else if skill.path.as_os_str().is_empty() {
            Some("Missing skill path".into())
        } else {
            None
        }
    });
    capability.register(provider("native", native_name, 100, native_skills));
    capability.register(provider("claude", "Claude Code", 80, claude_skills));
    capability.register(provider("agents", "Agent Dirs (.agent/.agents)", 70, agents_skills));
    capability.register(provider("codex", "OpenAI Codex", 70, codex_skills));
    capability.register(provider("opencode", "OpenCode", 55, opencode_skills));
    capability.register(provider("github", "GitHub Copilot", 30, github_skills));
    capability
}

/// `SkillsSettings` (`skills.*`), with upstream's defaults.
#[derive(Clone, Debug)]
pub struct SkillsSettings {
    pub enabled: bool,
    pub enable_codex_user: bool,
    pub enable_claude_user: bool,
    pub enable_claude_project: bool,
    /// Native user skills (upstream `enablePiUser`).
    pub enable_native_user: bool,
    /// Native project skills (upstream `enablePiProject`).
    pub enable_native_project: bool,
    pub enable_agents_user: bool,
    pub enable_agents_project: bool,
    pub custom_directories: Vec<String>,
    /// Glob patterns on skill names.
    pub ignored_skills: Vec<String>,
    pub include_skills: Vec<String>,
    pub disabled_extensions: Vec<String>,
}

impl Default for SkillsSettings {
    fn default() -> Self {
        SkillsSettings {
            enabled: true,
            enable_codex_user: false,
            enable_claude_user: false,
            enable_claude_project: true,
            enable_native_user: true,
            enable_native_project: true,
            enable_agents_user: true,
            enable_agents_project: true,
            custom_directories: Vec::new(),
            ignored_skills: Vec::new(),
            include_skills: Vec::new(),
            disabled_extensions: Vec::new(),
        }
    }
}

/// A loaded skill as the prompt and `skill://` see it.
#[derive(Clone, Debug, PartialEq)]
pub struct LoadedSkill {
    pub name: String,
    pub description: String,
    pub file_path: PathBuf,
    /// The skill's directory.
    pub base_dir: PathBuf,
    /// `<provider>:<level>` (`custom:user` for custom directories).
    pub source: String,
    /// Loaded but left out of the system prompt listing (`hide` or
    /// `disable-model-invocation`).
    pub hide: bool,
    pub meta: SourceMeta,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillWarning {
    pub skill_path: String,
    pub message: String,
}

fn loaded(skill: &Skill, source: String, meta: SourceMeta) -> LoadedSkill {
    let fm = &skill.frontmatter;
    LoadedSkill {
        name: skill.name.clone(),
        description: fm.get("description").and_then(Value::as_str).unwrap_or_default().to_string(),
        base_dir: skill.path.parent().map(Path::to_path_buf).unwrap_or_default(),
        file_path: skill.path.clone(),
        source,
        hide: fm.get("hide") == Some(&Value::Bool(true))
            || fm.get("disableModelInvocation") == Some(&Value::Bool(true)),
        meta,
    }
}

/// `loadSkillsFromDir`: one skills root, descriptions required. `source`
/// is `<provider>:<level>` (provider defaults to `custom`, level to `user`).
pub fn load_skills_from_dir(fs: &FsCache, dir: &Path, source: &str) -> (Vec<LoadedSkill>, Vec<SkillWarning>) {
    let mut parts = source.splitn(2, ':');
    let provider = parts.next().filter(|p| !p.is_empty()).unwrap_or("custom");
    let level = if parts.next() == Some("project") { Level::Project } else { Level::User };
    let scan = scan_skills_from_dir(fs, dir, provider, level, true);
    let skills = scan.items.iter().map(|skill| loaded(skill, source.to_string(), skill.source.clone())).collect();
    let warnings = scan
        .warnings
        .into_iter()
        .map(|message| SkillWarning { skill_path: dir.display().to_string(), message })
        .collect();
    (skills, warnings)
}

/// `~`, `~/x`, `~\x` and `~x` against `home` (`expandTilde`).
pub fn expand_tilde(path: &str, home: &Path) -> PathBuf {
    if path == "~" {
        return home.to_path_buf();
    }
    if path.starts_with("~/") || path.starts_with("~\\") {
        return PathBuf::from(format!("{}{}", home.display(), &path[1..]));
    }
    if let Some(rest) = path.strip_prefix('~') {
        return home.join(rest);
    }
    PathBuf::from(path)
}

fn glob_matches(patterns: &[String], name: &str) -> bool {
    patterns.iter().any(|p| globset::Glob::new(p).map(|g| g.compile_matcher().is_match(name)).unwrap_or(false))
}

impl Discovery {
    /// `loadSkills`: capability skills filtered by the source switches, name
    /// globs and disabled extensions (first per name, symlinked duplicates
    /// dropped), then custom directories, which win over default-path
    /// providers. Sorted by [`compare_skill_order`].
    pub fn load_skills(&self, cwd: &Path, settings: &SkillsSettings) -> (Vec<LoadedSkill>, Vec<SkillWarning>) {
        if !settings.enabled {
            return (Vec::new(), Vec::new());
        }
        let mut ctx = self.context(cwd);
        ctx.skill_toggles.claude_user = settings.enable_claude_user;
        ctx.skill_toggles.codex_user = settings.enable_codex_user;
        let options =
            LoadOptions { disabled_extensions: settings.disabled_extensions.clone(), ..LoadOptions::default() };
        let result = self.skills.load(&ctx, &options);

        let source_enabled = |meta: &SourceMeta| -> bool {
            match (meta.provider.as_str(), meta.level) {
                ("codex", Level::User) => settings.enable_codex_user || ctx.is_user_source_enabled("codex"),
                ("claude", Level::User) => settings.enable_claude_user || ctx.is_user_source_enabled("claude"),
                ("claude", Level::Project) => settings.enable_claude_project,
                ("native", Level::User) => settings.enable_native_user,
                ("native", Level::Project) => settings.enable_native_project,
                ("agents", Level::User) => settings.enable_agents_user,
                ("agents", Level::Project) => settings.enable_agents_project,
                (provider, Level::User) => ctx.is_user_source_enabled(provider),
                _ => true,
            }
        };
        let includes = |name: &str| settings.include_skills.is_empty() || glob_matches(&settings.include_skills, name);
        let ignored = |name: &str| glob_matches(&settings.ignored_skills, name);
        let disabled_names: HashSet<&str> =
            settings.disabled_extensions.iter().filter_map(|id| id.strip_prefix("skill:")).collect();

        let mut skills: Vec<LoadedSkill> = Vec::new();
        let mut real_paths: HashSet<PathBuf> = HashSet::new();
        let mut warnings: Vec<SkillWarning> = Vec::new();
        let mut seen_names: HashSet<String> = HashSet::new();
        let real = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());

        // From the pre-dedup superset: a disabled higher-priority provider must
        // not hide an enabled lower-priority skill of the same name.
        for (skill, _) in &result.all {
            if disabled_names.contains(skill.name.as_str())
                || !source_enabled(&skill.source)
                || ignored(&skill.name)
                || !includes(&skill.name)
                || !seen_names.insert(skill.name.clone())
            {
                continue;
            }
            let resolved = real(&skill.path);
            if real_paths.contains(&resolved) {
                continue;
            }
            if let Some(existing) = skills.iter().find(|s| s.name == skill.name) {
                warnings.push(SkillWarning {
                    skill_path: skill.path.display().to_string(),
                    message: format!(
                        "name collision: \"{}\" already loaded from {}, skipping this one",
                        skill.name,
                        existing.file_path.display()
                    ),
                });
                continue;
            }
            let source = format!("{}:{}", skill.source.provider, skill.level.as_str());
            skills.push(loaded(skill, source, skill.source.clone()));
            real_paths.insert(resolved);
        }

        // All custom directories are scanned first; their scan warnings precede
        // collision warnings, as upstream.
        let mut custom: Vec<Skill> = Vec::new();
        for dir in &settings.custom_directories {
            let expanded = expand_tilde(dir, &self.home);
            let scan = scan_skills_from_dir(&self.fs, &expanded, "custom", Level::User, true);
            custom.extend(scan.items.into_iter().filter(|skill| {
                !disabled_names.contains(skill.name.as_str()) && !ignored(&skill.name) && includes(&skill.name)
            }));
            warnings.extend(
                scan.warnings
                    .into_iter()
                    .map(|message| SkillWarning { skill_path: expanded.display().to_string(), message }),
            );
        }
        for skill in custom {
            let resolved = real(&skill.path);
            if real_paths.contains(&resolved) {
                continue;
            }
            let mut meta = skill.source.clone();
            meta.provider_name = "Custom".into();
            let entry = loaded(&skill, "custom:user".into(), meta);
            match skills.iter().position(|s| s.name == skill.name) {
                // A configured custom directory beats default-path providers.
                Some(index) if !skills[index].source.starts_with("custom:") => {
                    skills[index] = entry;
                    real_paths.insert(resolved);
                }
                Some(index) => warnings.push(SkillWarning {
                    skill_path: skill.path.display().to_string(),
                    message: format!(
                        "name collision: \"{}\" already loaded from {}, skipping this one",
                        skill.name,
                        skills[index].file_path.display()
                    ),
                }),
                None => {
                    skills.push(entry);
                    real_paths.insert(resolved);
                }
            }
        }

        skills.sort_by(|a, b| compare_skill_order(&a.name, &a.file_path, &b.name, &b.file_path));
        let mut all_warnings: Vec<SkillWarning> =
            result.warnings.into_iter().map(|message| SkillWarning { skill_path: String::new(), message }).collect();
        all_warnings.extend(warnings);
        (skills, all_warnings)
    }
}
