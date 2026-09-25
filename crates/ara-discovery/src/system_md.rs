//! `SYSTEM.md` customizations (`capability/system-prompt.ts`, the
//! `loadSystemPrompt` providers of `builtin`, `claude`, `agents`, `gemini`,
//! and `system-prompt.ts` `loadSystemPromptFiles`).

use crate::capability::{Capability, Level, LoadContext, LoadOptions, LoadResult, Provider, SourceMeta, Sourced};
use crate::context_files::nearest_native_project_dir;
use crate::project::Discovery;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A system prompt customization file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemMd {
    pub path: PathBuf,
    pub content: String,
    pub level: Level,
    pub source: SourceMeta,
}

impl Sourced for SystemMd {
    fn source(&self) -> &SourceMeta {
        &self.source
    }
    fn source_mut(&mut self) -> &mut SourceMeta {
        &mut self.source
    }
}

type Items = Result<LoadResult<SystemMd>, String>;

fn item(provider: &str, path: PathBuf, content: String, level: Level) -> SystemMd {
    SystemMd { source: SourceMeta::new(provider, &path, level), path, content, level }
}

fn truthy(ctx: &LoadContext<'_>, path: &Path) -> Option<String> {
    ctx.fs.read_file(path).filter(|c| !c.is_empty())
}

fn native(ctx: &LoadContext<'_>) -> Items {
    let mut items = Vec::new();
    let user = ctx.dirs.native_user_dir.join("SYSTEM.md");
    if let Some(content) = truthy(ctx, &user) {
        items.push(item("native", user, content, Level::User));
    }
    if let Some((dir, _)) = nearest_native_project_dir(ctx) {
        let path = dir.join("SYSTEM.md");
        if let Some(content) = truthy(ctx, &path) {
            items.push(item("native", path, content, Level::Project));
        }
    }
    Ok(LoadResult { items, warnings: Vec::new() })
}

fn claude(ctx: &LoadContext<'_>) -> Items {
    let mut items = Vec::new();
    if ctx.is_user_source_enabled("claude") {
        let dir = ctx.dirs.claude_config_dir.clone().unwrap_or_else(|| ctx.home.join(".claude"));
        let path = dir.join("SYSTEM.md");
        // Upstream keeps an empty Claude file (`!== null`).
        if let Some(content) = ctx.fs.read_file(&path) {
            items.push(item("claude", path, content, Level::User));
        }
    }
    Ok(LoadResult { items, warnings: Vec::new() })
}

fn agents(ctx: &LoadContext<'_>) -> Items {
    let mut candidates: Vec<(PathBuf, Level)> = Vec::new();
    let stop = ctx.repo_root.clone().unwrap_or_else(|| ctx.home.clone());
    let mut current = ctx.cwd.clone();
    loop {
        if current != ctx.home {
            for base in [".agent", ".agents"] {
                candidates.push((current.join(base).join("SYSTEM.md"), Level::Project));
            }
        }
        if current == stop {
            break;
        }
        let Some(parent) = current.parent() else { break };
        current = parent.to_path_buf();
    }
    let mut homes = vec![ctx.home.clone()];
    homes.extend(ctx.dirs.extra_user_homes.iter().filter(|h| **h != ctx.home).cloned());
    for home in homes {
        for base in [".agent", ".agents"] {
            candidates.push((home.join(base).join("SYSTEM.md"), Level::User));
        }
    }
    let items = candidates
        .into_iter()
        .filter_map(|(path, level)| truthy(ctx, &path).map(|content| item("agents", path, content, level)))
        .collect();
    Ok(LoadResult { items, warnings: Vec::new() })
}

fn gemini(ctx: &LoadContext<'_>) -> Items {
    let mut items = Vec::new();
    if ctx.is_user_source_enabled("gemini") {
        let path = ctx.home.join(".gemini/system.md");
        if let Some(content) = truthy(ctx, &path) {
            items.push(item("gemini", path, content, Level::User));
        }
    }
    let path = ctx.cwd.join(".gemini/system.md");
    if let Some(content) = truthy(ctx, &path) {
        items.push(item("gemini", path, content, Level::Project));
    }
    Ok(LoadResult { items, warnings: Vec::new() })
}

fn provider(id: &str, name: &str, priority: i32, load: fn(&LoadContext<'_>) -> Items) -> Provider<SystemMd> {
    Provider {
        id: id.into(),
        display_name: name.into(),
        description: format!("SYSTEM.md from {name}"),
        priority,
        load: Arc::new(load),
    }
}

/// The system-prompt capability: one file per level, first provider wins.
pub fn system_md_capability(native_name: &str) -> Capability<SystemMd> {
    let mut capability = Capability::new(
        "system-prompt",
        "System Prompt",
        "Custom system prompt files (SYSTEM.md) that modify agent behavior",
        |sp: &SystemMd| Some(sp.level.as_str().to_string()),
    );
    capability.validate = Some(|sp: &SystemMd| sp.path.as_os_str().is_empty().then(|| "Missing path".to_string()));
    capability.register(provider("native", native_name, 100, native));
    capability.register(provider("claude", "Claude Code", 80, claude));
    capability.register(provider("agents", "Agent Dirs (.agent/.agents)", 70, agents));
    capability.register(provider("gemini", "Gemini CLI", 60, gemini));
    capability
}

impl Discovery {
    /// `loadSystemPromptFiles`: the project-level SYSTEM.md, else the user one.
    pub fn load_system_prompt_file(&self, cwd: &Path) -> Option<SystemMd> {
        let capability = system_md_capability(&self.dirs.native_name);
        let result = capability.load(&self.context(cwd), &LoadOptions::default());
        let project = result.items.iter().find(|i| i.level == Level::Project).cloned();
        project.or_else(|| result.items.into_iter().find(|i| i.level == Level::User))
    }
}
