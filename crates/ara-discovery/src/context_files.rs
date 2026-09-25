//! Context files (`capability/context-file.ts`) and the providers that find
//! them (`discovery/{builtin,claude,agents,codex,gemini,opencode,github,
//! agents-md,claude-md}.ts`, `helpers.ts` `loadStandaloneContextFiles`).

use crate::capability::{Capability, Level, LoadContext, LoadResult, Provider, SourceMeta, Sourced};
use crate::paths::{calculate_depth, is_within, resolve, same_path};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A persistent instruction file (AGENTS.md, CLAUDE.md, GEMINI.md, …).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextFile {
    pub path: PathBuf,
    pub content: String,
    /// `User` or `Project`.
    pub level: Level,
    /// Distance from cwd for project files (0 = cwd, 1 = parent; config
    /// subdirectories of cwd such as `.gemini/` are -1).
    pub depth: Option<i64>,
    pub source: SourceMeta,
}

impl Sourced for ContextFile {
    fn source(&self) -> &SourceMeta {
        &self.source
    }
    fn source_mut(&mut self) -> &mut SourceMeta {
        &mut self.source
    }
}

impl ContextFile {
    fn new(provider: &str, path: PathBuf, content: String, level: Level, depth: Option<i64>) -> Self {
        let source = SourceMeta::new(provider, &path, level);
        ContextFile { path, content, level, depth, source }
    }
}

/// One user-level file, and one project-level file per depth (clamped at 0
/// so config subdirectories share their directory's scope).
fn key(file: &ContextFile) -> Option<String> {
    Some(match file.level {
        Level::User => "user".into(),
        _ => format!("project:{}", file.depth.unwrap_or(0).max(0)),
    })
}

fn validate(file: &ContextFile) -> Option<String> {
    if file.path.as_os_str().is_empty() {
        return Some("Missing path".into());
    }
    if !matches!(file.level, Level::User | Level::Project) {
        return Some("Invalid level: must be 'user' or 'project'".into());
    }
    None
}

fn to_extension_id(file: &ContextFile) -> Option<String> {
    let base = file.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Some(format!("context-file:{}:{base}", file.level.as_str()))
}

type Items = Result<LoadResult<ContextFile>, String>;

fn ok(items: Vec<ContextFile>) -> Items {
    Ok(LoadResult { items, warnings: Vec::new() })
}

/// Non-empty file text (upstream's truthiness check).
fn non_empty(ctx: &LoadContext<'_>, path: &Path) -> Option<String> {
    ctx.fs.read_file(path).filter(|c| !c.is_empty())
}

// --- native (.ara / ~/.ara/agent) -------------------------------------------

fn native_context_files(ctx: &LoadContext<'_>) -> Items {
    let mut items = Vec::new();
    let user = ctx.dirs.native_user_dir.join("AGENTS.md");
    if let Some(content) = non_empty(ctx, &user) {
        items.push(ContextFile::new("native", user, content, Level::User, None));
    }
    // Nearest non-empty project config dir from cwd up to the repo root.
    let mut current = ctx.cwd.clone();
    let mut depth = 0;
    loop {
        let dir = current.join(&ctx.dirs.native_project_dir);
        if !ctx.fs.read_dir_entries(&dir).is_empty() {
            let path = resolve(&dir).join("AGENTS.md");
            if let Some(content) = non_empty(ctx, &path) {
                items.push(ContextFile::new("native", path, content, Level::Project, Some(depth)));
            }
            break;
        }
        if ctx.repo_root.as_deref() == Some(current.as_path()) {
            break;
        }
        let Some(parent) = current.parent() else { break };
        current = parent.to_path_buf();
        depth += 1;
    }
    ok(items)
}

// --- Claude Code (~/.claude, .claude/) ----------------------------------------

fn claude_context_files(ctx: &LoadContext<'_>) -> Items {
    let mut items = Vec::new();
    if ctx.is_user_source_enabled("claude") {
        let dir = ctx.dirs.claude_config_dir.clone().unwrap_or_else(|| ctx.home.join(".claude"));
        let path = dir.join("CLAUDE.md");
        // Upstream keeps empty Claude files (`!== null`).
        if let Some(content) = ctx.fs.read_file(&path) {
            items.push(ContextFile::new("claude", path, content, Level::User, None));
        }
    }
    let base = ctx.cwd.join(".claude");
    let path = base.join("CLAUDE.md");
    if let Some(content) = ctx.fs.read_file(&path) {
        let depth = calculate_depth(&ctx.cwd, base.parent().unwrap_or(&ctx.cwd));
        items.push(ContextFile::new("claude", path, content, Level::Project, Some(depth)));
    }
    ok(items)
}

// --- .agent / .agents ---------------------------------------------------------

const AGENT_DIRS: [&str; 2] = [".agent", ".agents"];

fn agents_context_files(ctx: &LoadContext<'_>) -> Items {
    let mut candidates: Vec<(PathBuf, Level)> = Vec::new();
    // Project: from cwd up to the repo root (or home), skipping home itself.
    let stop = ctx.repo_root.clone().unwrap_or_else(|| ctx.home.clone());
    let mut current = ctx.cwd.clone();
    loop {
        if current != ctx.home {
            for dir in AGENT_DIRS {
                candidates.push((current.join(dir).join("AGENTS.md"), Level::Project));
            }
        }
        if current == stop {
            break;
        }
        let Some(parent) = current.parent() else { break };
        current = parent.to_path_buf();
    }
    let mut homes = vec![ctx.home.clone()];
    for extra in &ctx.dirs.extra_user_homes {
        if !homes.contains(extra) {
            homes.push(extra.clone());
        }
    }
    for home in homes {
        for dir in AGENT_DIRS {
            candidates.push((home.join(dir).join("AGENTS.md"), Level::User));
        }
    }
    let items = candidates
        .into_iter()
        .filter_map(|(path, level)| {
            let content = non_empty(ctx, &path)?;
            let depth = (level == Level::Project).then(|| {
                let ancestor = path.parent().and_then(Path::parent).unwrap_or(Path::new("/"));
                calculate_depth(&ctx.cwd, ancestor)
            });
            Some(ContextFile::new("agents", path, content, level, depth))
        })
        .collect();
    ok(items)
}

// --- user-only foreign tools ------------------------------------------------

fn user_file(ctx: &LoadContext<'_>, provider: &str, rel: &[&str]) -> Option<ContextFile> {
    if !ctx.is_user_source_enabled(provider) {
        return None;
    }
    let path = rel.iter().fold(ctx.home.clone(), |p, s| p.join(s));
    let content = non_empty(ctx, &path)?;
    Some(ContextFile::new(provider, path, content, Level::User, None))
}

fn codex_context_files(ctx: &LoadContext<'_>) -> Items {
    ok(user_file(ctx, "codex", &[".codex", "AGENTS.md"]).into_iter().collect())
}

fn opencode_context_files(ctx: &LoadContext<'_>) -> Items {
    ok(user_file(ctx, "opencode", &[".config", "opencode", "AGENTS.md"]).into_iter().collect())
}

fn gemini_context_files(ctx: &LoadContext<'_>) -> Items {
    let mut items: Vec<ContextFile> = user_file(ctx, "gemini", &[".gemini", "GEMINI.md"]).into_iter().collect();
    let base = ctx.cwd.join(".gemini");
    let path = base.join("GEMINI.md");
    if let Some(content) = non_empty(ctx, &path) {
        let depth = calculate_depth(&ctx.cwd, &base);
        items.push(ContextFile::new("gemini", path, content, Level::Project, Some(depth)));
    }
    ok(items)
}

fn github_context_files(ctx: &LoadContext<'_>) -> Items {
    let mut items = Vec::new();
    let base = ctx.cwd.join(".github");
    let path = base.join("copilot-instructions.md");
    if let Some(content) = non_empty(ctx, &path) {
        let depth = calculate_depth(&ctx.cwd, &base);
        items.push(ContextFile::new("github", path, content, Level::Project, Some(depth)));
    }
    if ctx.is_user_source_enabled("github") {
        let home = ctx.dirs.copilot_home.clone().unwrap_or_else(|| ctx.home.join(".copilot"));
        let path = home.join("copilot-instructions.md");
        if let Some(content) = non_empty(ctx, &path) {
            items.push(ContextFile::new("github", path, content, Level::User, None));
        }
    }
    for dir in &ctx.dirs.copilot_custom_instruction_dirs {
        let path = dir.join("AGENTS.md");
        if let Some(content) = non_empty(ctx, &path) {
            items.push(ContextFile::new("github", path, content, Level::User, None));
        }
    }
    ok(items)
}

// --- standalone AGENTS.md / CLAUDE.md -----------------------------------------

/// `loadStandaloneContextFiles`: walk up from cwd collecting `file_name`.
///
/// Inside a repository nested below home the walk continues past the repo
/// root to pick up workspace files but never loads home's own copy; a repo
/// rooted at home keeps home's file as project context; outside any repo
/// the walk stops at home (inclusive) or the filesystem root (exclusive).
pub fn load_standalone_context_files(ctx: &LoadContext<'_>, provider: &str, file_name: &str) -> Vec<ContextFile> {
    let home = resolve(&ctx.home);
    let cwd = resolve(&ctx.cwd);
    let repo_root = ctx.repo_root.as_deref().map(resolve);
    let filesystem_root = cwd.ancestors().last().unwrap_or(Path::new("/")).to_path_buf();
    let cwd_under_home = is_within(&home, &cwd);
    let repo_is_home = repo_root.as_deref().is_some_and(|r| same_path(&home, r));
    let repo_under_home = repo_root.as_deref().is_some_and(|r| is_within(&home, r)) && !repo_is_home;
    let scan_to_home = repo_root.is_some() && cwd_under_home && repo_under_home;
    let boundary = if scan_to_home {
        home.clone()
    } else {
        repo_root.clone().unwrap_or_else(|| if cwd_under_home { home.clone() } else { filesystem_root })
    };
    let include_boundary = match repo_root {
        None => cwd_under_home,
        Some(_) => !same_path(&boundary, &home) || repo_is_home,
    };

    let mut items = Vec::new();
    let mut current = cwd.clone();
    loop {
        let at_boundary = same_path(&current, &boundary);
        let at_home = scan_to_home && same_path(&current, &home);
        if !(at_home || (at_boundary && !include_boundary)) {
            let candidate = current.join(file_name);
            // Empty files contribute nothing and must not claim a depth scope.
            if let Some(content) = non_empty(ctx, &candidate) {
                let hidden = current.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'));
                if !hidden {
                    let depth = calculate_depth(&cwd, &current);
                    items.push(ContextFile::new(provider, candidate, content, Level::Project, Some(depth)));
                }
            }
        }
        if at_boundary {
            break;
        }
        let Some(parent) = current.parent() else { break };
        current = parent.to_path_buf();
    }
    items
}

fn provider(
    id: &str,
    display_name: &str,
    description: &str,
    priority: i32,
    load: impl Fn(&LoadContext<'_>) -> Items + Send + Sync + 'static,
) -> Provider<ContextFile> {
    Provider {
        id: id.into(),
        display_name: display_name.into(),
        description: description.into(),
        priority,
        load: Arc::new(load),
    }
}

/// The context-files capability with upstream's providers, registered in
/// upstream's import order (ties keep it: AGENTS.md before CLAUDE.md,
/// `.agents` before Codex).
pub fn context_file_capability(native_name: &str) -> Capability<ContextFile> {
    let mut capability = Capability::new(
        "context-files",
        "Context Files",
        "Persistent instruction files (CLAUDE.md, AGENTS.md, etc.) that guide agent behavior",
        key,
    );
    capability.validate = Some(validate);
    capability.to_extension_id = Some(to_extension_id);
    capability.register(provider(
        "agents-md",
        "AGENTS.md",
        "Standalone AGENTS.md files (Codex/Gemini style)",
        10,
        |ctx| ok(load_standalone_context_files(ctx, "agents-md", "AGENTS.md")),
    ));
    capability.register(provider(
        "claude-md",
        "CLAUDE.md",
        "Standalone CLAUDE.md files (Claude Code style)",
        10,
        |ctx| ok(load_standalone_context_files(ctx, "claude-md", "CLAUDE.md")),
    ));
    capability.register(provider(
        "native",
        native_name,
        "Load AGENTS.md from native config directories",
        100,
        native_context_files,
    ));
    capability.register(provider(
        "claude",
        "Claude Code",
        "Load CLAUDE.md from ~/.claude and .claude/",
        80,
        claude_context_files,
    ));
    capability.register(provider(
        "agents",
        "Agent Dirs (.agent/.agents)",
        "Load AGENTS.md from .agent/ and .agents/",
        70,
        agents_context_files,
    ));
    capability.register(provider("codex", "OpenAI Codex", "Load AGENTS.md from ~/.codex", 70, codex_context_files));
    capability.register(provider(
        "gemini",
        "Gemini CLI",
        "Load GEMINI.md from ~/.gemini and .gemini/",
        60,
        gemini_context_files,
    ));
    capability.register(provider(
        "opencode",
        "OpenCode",
        "Load AGENTS.md from ~/.config/opencode",
        55,
        opencode_context_files,
    ));
    capability.register(provider(
        "github",
        "GitHub Copilot",
        "Load copilot-instructions.md from .github/ and ~/.copilot/; AGENTS.md from COPILOT_CUSTOM_INSTRUCTIONS_DIRS",
        30,
        github_context_files,
    ));
    capability
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_validation() {
        let file = |level, depth| ContextFile::new("t", PathBuf::from("/r/AGENTS.md"), "x".into(), level, depth);
        assert_eq!(key(&file(Level::User, Some(3))).unwrap(), "user");
        assert_eq!(key(&file(Level::Project, Some(-1))).unwrap(), "project:0");
        assert_eq!(key(&file(Level::Project, None)), key(&file(Level::Project, Some(0))));
        assert_ne!(key(&file(Level::Project, Some(1))), key(&file(Level::Project, Some(2))));
        let mut empty = file(Level::Project, Some(0));
        empty.path = PathBuf::new();
        assert_eq!(validate(&empty).as_deref(), Some("Missing path"));
        assert_eq!(validate(&file(Level::Project, Some(0))), None);
        assert_eq!(to_extension_id(&file(Level::User, None)).unwrap(), "context-file:user:AGENTS.md");
        let order: Vec<_> = context_file_capability("ARA").providers().iter().map(|p| p.id.clone()).collect();
        assert_eq!(
            order,
            ["native", "claude", "agents", "codex", "gemini", "opencode", "github", "agents-md", "claude-md"]
        );
    }
}
