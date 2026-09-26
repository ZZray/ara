#![recursion_limit = "256"]
//! System prompt assembly for the ARA Agent Core (OMP
//! `packages/coding-agent/src/system-prompt.ts` `buildSystemPrompt`,
//! `session/date-cwd-reminder.ts`, `utils/active-repo-context.ts` at
//! 596f2da7101178214aa27a753529d15e6b7ad91d), rendering upstream's templates
//! (see `prompts/README.md` for ARA's edits) with `ara-prompt`.
//!
//! Not ported here: the full `namespace functions` tool catalog (ARA uses
//! provider-native tool calling, so the compact list mode applies), GPU
//! probing, workspace-tree building, rules, subagent/delegation settings and
//! `xd://` devices; the templates' sections for them stay off.

mod reminder;

pub use reminder::{DateCwdReminder, render_date_cwd_reminder};

use ara_discovery::{Discovery, LoadedSkill, ProjectContextFile, SkillsSettings, dedupe_contained_context_files};
use ara_prompt::TemplateError;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const SYSTEM_TEMPLATE: &str = include_str!("../prompts/system-prompt.md");
const PROJECT_TEMPLATE: &str = include_str!("../prompts/project-prompt.md");
const CUSTOM_TEMPLATE: &str = include_str!("../prompts/custom-system-prompt.md");
const ACTIVE_REPO_TEMPLATE: &str = include_str!("../prompts/active-repo-context.md");

/// Tool names upstream falls back to when no tool list is given.
pub const DEFAULT_TOOL_NAMES: [&str; 4] = ["read", "bash", "edit", "write"];

/// Bundled personality presets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Personality {
    #[default]
    Default,
    Friendly,
    Pragmatic,
    /// No personality block (subagents).
    None,
}

impl Personality {
    fn preset(self) -> &'static str {
        match self {
            Personality::Default => include_str!("../prompts/personalities/default.md"),
            Personality::Friendly => include_str!("../prompts/personalities/friendly.md"),
            Personality::Pragmatic => include_str!("../prompts/personalities/pragmatic.md"),
            Personality::None => "",
        }
    }
}

/// A tool as the prompt's compact inventory lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptTool {
    pub name: String,
    pub label: String,
}

/// Internal URL schemes the host's tools resolve (ARA edit 2 in
/// `prompts/README.md`): only these are advertised.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InternalUrls {
    pub skill: bool,
    pub rule: bool,
    pub agent: bool,
    pub history: bool,
    pub artifact: bool,
    pub local: bool,
    pub mcp: bool,
    pub github: bool,
    /// Harness documentation URL (upstream `omp://`).
    pub harness_docs: Option<String>,
}

impl InternalUrls {
    fn to_json(&self) -> Value {
        let any = self.skill
            || self.rule
            || self.agent
            || self.history
            || self.artifact
            || self.local
            || self.mcp
            || self.github
            || self.harness_docs.is_some();
        json!({
            "any": any, "skill": self.skill, "rule": self.rule, "agent": self.agent, "history": self.history,
            "artifact": self.artifact, "local": self.local, "mcp": self.mcp, "github": self.github,
            "harnessDocs": self.harness_docs,
        })
    }
}

/// A rendered workspace tree supplied by the host.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceTree {
    pub rendered: String,
    pub truncated: bool,
    pub agents_md_files: Vec<String>,
}

/// `BuildSystemPromptOptions` (the ported subset).
#[derive(Clone, Debug)]
pub struct SystemPromptOptions {
    /// Harness name for the role line (`ARA`).
    pub harness_name: String,
    /// Replaces the default template (already resolved text; see
    /// [`resolve_prompt_input`]). Suppresses discovered `SYSTEM.md`.
    pub custom_prompt: Option<String>,
    pub append_prompt: Option<String>,
    /// Active tools in order; `None` falls back to [`DEFAULT_TOOL_NAMES`].
    pub tools: Option<Vec<PromptTool>>,
    /// Pre-loaded context files; `None` discovers them from cwd.
    pub context_files: Option<Vec<ProjectContextFile>>,
    /// Pre-loaded skills; `None` loads them with `skills_settings`.
    pub skills: Option<Vec<LoadedSkill>>,
    pub skills_settings: SkillsSettings,
    /// `disabledExtensions` for context-file discovery.
    pub disabled_extensions: Vec<String>,
    pub additional_workspace_roots: Vec<PathBuf>,
    pub model: Option<String>,
    pub include_model_in_prompt: bool,
    pub personality: Personality,
    /// Pre-resolved active child repo (relative path); `None` resolves from cwd.
    pub active_repo_context: Option<Option<String>>,
    pub include_workspace_tree: bool,
    pub workspace_tree: Option<WorkspaceTree>,
    pub render_mermaid: bool,
    pub reactions: bool,
    pub urls: InternalUrls,
}

impl Default for SystemPromptOptions {
    fn default() -> Self {
        SystemPromptOptions {
            harness_name: "ARA".into(),
            custom_prompt: None,
            append_prompt: None,
            tools: None,
            context_files: None,
            skills: None,
            skills_settings: SkillsSettings::default(),
            disabled_extensions: Vec::new(),
            additional_workspace_roots: Vec::new(),
            model: None,
            include_model_in_prompt: true,
            personality: Personality::Default,
            active_repo_context: None,
            include_workspace_tree: false,
            workspace_tree: None,
            render_mermaid: true,
            reactions: false,
            urls: InternalUrls::default(),
        }
    }
}

/// `resolvePromptInput`: text with a newline is literal; otherwise a
/// readable file's content (decoded lossily, as `Bun.file().text()`), else the
/// text itself. A read failure other than a missing file or an over-long name
/// adds a warning (upstream logs it) and still falls back to the text.
pub fn resolve_prompt_input(input: Option<&str>, description: &str, warnings: &mut Vec<String>) -> Option<String> {
    let input = input.filter(|s| !s.is_empty())?;
    if input.contains('\n') {
        return Some(input.to_string());
    }
    match std::fs::read(input) {
        Ok(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
        Err(e) => {
            let quiet = e.kind() == std::io::ErrorKind::NotFound || e.raw_os_error() == Some(libc::ENAMETOOLONG);
            if !quiet {
                warnings.push(format!("Could not read {description} file {input}: {e}"));
            }
            Some(input.to_string())
        }
    }
}

/// `loadPersonalityOverride`: the trimmed `PERSONALITY.md`, or `None` with a
/// warning when it is empty or unreadable (a missing file is silent).
fn load_personality_override(path: &Path, warnings: &mut Vec<String>) -> Option<String> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let content = String::from_utf8_lossy(&bytes).trim().to_string();
            if content.is_empty() {
                warnings.push(format!(
                    "PERSONALITY.md is empty; using the configured personality preset ({})",
                    path.display()
                ));
                return None;
            }
            Some(content)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            warnings.push(format!(
                "Failed to read PERSONALITY.md; using the configured personality preset ({}): {e}",
                path.display()
            ));
            None
        }
    }
}

/// A built system prompt and the warnings met while building it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SystemPrompt {
    /// Ordered system prompt blocks.
    pub blocks: Vec<String>,
    /// Non-fatal problems (unreadable `PERSONALITY.md`, …) for the host to
    /// surface.
    pub warnings: Vec<String>,
}

fn first_non_empty(value: Option<&str>) -> Option<&str> {
    value.map(ara_prompt::js::trim).filter(|s| !s.is_empty())
}

fn prompt_source_contains(source: Option<&str>, rule: &str) -> bool {
    let source = ara_discovery::project::split_comparable_prompt_blocks(source.unwrap_or_default());
    let rule = ara_discovery::project::split_comparable_prompt_blocks(rule);
    ara_discovery::project::prompt_blocks_contain(&source, &rule)
}

/// `dedupePromptSource`: drop `source` when another source contains it.
fn dedupe_prompt_source(source: Option<&str>, others: &[Option<&str>]) -> String {
    let Some(resolved) = first_non_empty(source) else { return String::new() };
    if others.iter().any(|other| prompt_source_contains(*other, resolved)) {
        String::new()
    } else {
        resolved.to_string()
    }
}

/// `resolveActiveRepoContext`: outside any repository, the one direct child
/// directory holding `.git` (relative path), if exactly one does.
pub fn resolve_active_repo_context(discovery: &Discovery, cwd: &Path) -> Option<String> {
    let cwd = ara_discovery::paths::resolve(cwd);
    if discovery.fs.find_repo_root(&cwd).is_some() {
        return None;
    }
    let mut entries: Vec<_> = std::fs::read_dir(&cwd).ok()?.filter_map(Result::ok).collect();
    entries.sort_by(|a, b| {
        a.file_name().to_string_lossy().encode_utf16().cmp(b.file_name().to_string_lossy().encode_utf16())
    });
    let mut found: Option<String> = None;
    for entry in entries {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else { continue };
        let is_dir = kind.is_dir() || (kind.is_symlink() && path.is_dir());
        if !is_dir {
            continue;
        }
        let git = path.join(".git");
        if !(git.is_dir() || git.is_file()) {
            continue;
        }
        if found.is_some() {
            return None;
        }
        found = Some(entry.file_name().to_string_lossy().into_owned());
    }
    found
}

fn normalize_prompt_path(value: &str) -> String {
    value.replace('\\', "/")
}

/// `getEnvironmentInfo` (without the GPU probe).
pub fn environment_info() -> Vec<(String, String)> {
    let (sysname, release, version) = uname();
    let platform = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "x86" => "ia32",
        other => other,
    };
    let kernel = kernel_identity(&version, &sysname, &release);
    let terminal = std::env::var("TERM_PROGRAM")
        .ok()
        .filter(|t| !t.is_empty())
        .map(|t| match std::env::var("TERM_PROGRAM_VERSION") {
            Ok(v) if !v.is_empty() => format!("{t} {v}"),
            _ => t,
        })
        .or_else(|| std::env::var("WT_SESSION").ok().map(|_| "Windows Terminal".to_string()))
        .or_else(|| {
            ["TERM", "COLORTERM", "TERMINAL_EMULATOR"]
                .iter()
                .find_map(|k| std::env::var(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty()))
        });
    let entries = [
        ("OS", Some(format!("{platform} {release}"))),
        ("Distro", Some(sysname)),
        ("Kernel", Some(kernel)),
        ("Arch", Some(arch.to_string())),
        ("CPU", cpu_model()),
        ("Terminal", terminal),
    ];
    entries
        .into_iter()
        .filter_map(|(label, value)| value.filter(|v| !v.is_empty()).map(|v| (label.to_string(), v)))
        .collect()
}

/// `getKernelIdentity`: the uname build string, or `<type> <release>` when
/// it is empty or `unknown`.
pub fn kernel_identity(version: &str, sysname: &str, release: &str) -> String {
    let version = version.trim();
    if !version.is_empty() && !version.eq_ignore_ascii_case("unknown") {
        version.to_string()
    } else {
        format!("{sysname} {release}").trim().to_string()
    }
}

#[cfg(unix)]
fn uname() -> (String, String, String) {
    // SAFETY: `uname` fills a zeroed struct; the fields are NUL-terminated.
    unsafe {
        let mut buf: libc::utsname = std::mem::zeroed();
        if libc::uname(&mut buf) != 0 {
            return (String::new(), String::new(), String::new());
        }
        let field = |f: &[libc::c_char]| std::ffi::CStr::from_ptr(f.as_ptr()).to_string_lossy().into_owned();
        (field(&buf.sysname), field(&buf.release), field(&buf.version))
    }
}

#[cfg(not(unix))]
fn uname() -> (String, String, String) {
    (std::env::consts::OS.to_string(), String::new(), String::new())
}

fn cpu_model() -> Option<String> {
    let info = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    info.lines()
        .find_map(|line| line.strip_prefix("model name").and_then(|rest| rest.trim_start().strip_prefix(':')))
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// `buildSystemPrompt`: ordered system prompt blocks plus warnings.
pub fn build_system_prompt(
    discovery: &Discovery,
    cwd: &Path,
    options: &SystemPromptOptions,
) -> Result<SystemPrompt, TemplateError> {
    let mut warnings = Vec::new();
    let cwd = ara_discovery::paths::resolve(cwd);
    let custom = options.custom_prompt.as_deref().filter(|s| !s.is_empty());
    let append = options.append_prompt.as_deref();

    // A caller-supplied custom prompt owns block 0; discovered SYSTEM.md stays out.
    let system_md = if custom.is_some() { None } else { discovery.load_system_prompt_file(&cwd).map(|f| f.content) };

    // Supplied or discovered, the cwd's files are joined by every extra
    // workspace root's own, then deduped (upstream `contextFilesPromise`).
    let mut files = match &options.context_files {
        Some(files) => files.clone(),
        None => discovery.load_project_context_files(&cwd, &options.disabled_extensions),
    };
    for root in &options.additional_workspace_roots {
        if ara_discovery::paths::resolve(root) != cwd {
            files.extend(discovery.load_project_context_files(root, &options.disabled_extensions));
        }
    }
    let context_files = dedupe_contained_context_files(files);
    let skills = match &options.skills {
        Some(skills) => skills.clone(),
        None if options.skills_settings.enabled => discovery.load_skills(&cwd, &options.skills_settings).0,
        None => Vec::new(),
    };
    let active_repo = match &options.active_repo_context {
        Some(resolved) => resolved.clone(),
        None => resolve_active_repo_context(discovery, &cwd),
    };
    let personality = match options.personality {
        Personality::None => String::new(),
        preset => {
            let override_path = discovery.dirs.native_user_dir.join("PERSONALITY.md");
            load_personality_override(&override_path, &mut warnings)
                .unwrap_or_else(|| preset.preset().trim().to_string())
        }
    };

    let tools: Vec<PromptTool> = match &options.tools {
        Some(tools) => tools.clone(),
        None => DEFAULT_TOOL_NAMES.iter().map(|n| PromptTool { name: (*n).into(), label: String::new() }).collect(),
    };
    let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    let tool_refs: serde_json::Map<String, Value> =
        tools.iter().map(|t| (t.name.clone(), Value::String(t.name.clone()))).collect();
    let tool_info: Vec<Value> =
        tools.iter().map(|t| json!({ "name": t.name, "internalName": t.name, "label": t.label })).collect();

    // Skills need `read` to be usable; hidden skills stay out of the listing.
    let has_read = tool_names.contains(&"read");
    let listed_skills: Vec<Value> = if has_read {
        skills.iter().filter(|s| !s.hide).map(|s| json!({ "name": s.name, "description": s.description })).collect()
    } else {
        Vec::new()
    };

    let customization = dedupe_prompt_source(system_md.as_deref(), &[custom, append]);
    let workspace_tree = options.workspace_tree.clone().unwrap_or_default();
    let mut agents_md_files = workspace_tree.agents_md_files.clone();
    agents_md_files.sort();
    agents_md_files.dedup();
    agents_md_files.truncate(200);
    let environment: Vec<Value> =
        environment_info().into_iter().map(|(label, value)| json!({ "label": label, "value": value })).collect();
    let context_json: Vec<Value> = context_files
        .iter()
        .map(|f| json!({ "path": normalize_prompt_path(&f.path.display().to_string()), "content": f.content, "depth": f.depth }))
        .collect();
    let additional_roots: Vec<String> = options
        .additional_workspace_roots
        .iter()
        .filter(|r| ara_discovery::paths::resolve(r) != cwd)
        .map(|r| r.display().to_string())
        .collect();

    let mut data = json!({
        "harnessName": options.harness_name,
        "systemPromptCustomization": customization,
        "customPrompt": custom,
        "appendPrompt": append.unwrap_or_default(),
        "tools": tool_names,
        "toolInfo": tool_info,
        "toolInventory": "",
        "inlineToolDescriptors": false,
        "toolListMode": true,
        "toolRefs": tool_refs,
        "environment": environment,
        "contextFiles": context_json,
        "agentsMdSearch": { "files": agents_md_files },
        "workspaceTree": { "rendered": workspace_tree.rendered, "truncated": workspace_tree.truncated },
        "skills": listed_skills,
        "rules": [],
        "alwaysApplyRules": [],
        "cwd": normalize_prompt_path(&cwd.display().to_string()),
        "additionalWorkspaceRoots": additional_roots,
        "model": if options.include_model_in_prompt { options.model.clone().unwrap_or_default() } else { String::new() },
        "useCodexTaskPrompt": false,
        "personality": personality,
        "intentTracing": false,
        "intentField": "",
        "eagerTasks": false,
        "eagerTasksAlways": false,
        "taskBatch": true,
        "MAX_CONCURRENCY": 0,
        "scoutAvailable": false,
        "taskIrcEnabled": false,
        "secretsEnabled": false,
        "hasMemoryRoot": false,
        "securityEnabled": false,
        "hasObsidian": false,
        "includeWorkspaceTree": options.include_workspace_tree,
        "renderMermaid": options.render_mermaid,
        "reactions": options.reactions,
        "xdevTools": [],
        "hasDynamicXdevTools": false,
        "xdevDocs": "",
        "autoQaEnabled": false,
        "writeTransportOnly": false,
        "urls": options.urls.to_json(),
    });

    let main = ara_prompt::render(if custom.is_some() { CUSTOM_TEMPLATE } else { SYSTEM_TEMPLATE }, &data)?;
    let mut blocks = vec![main];
    // Custom templates already carry context files and the append text.
    if custom.is_some() {
        data["contextFiles"] = json!([]);
        data["appendPrompt"] = json!("");
    }
    let project = ara_prompt::render(PROJECT_TEMPLATE, &data)?;
    let project = ara_prompt::js::trim(&project);
    if !project.is_empty() {
        blocks.push(project.to_string());
    }
    if let Some(relative) = active_repo {
        let rendered =
            ara_prompt::render(ACTIVE_REPO_TEMPLATE, &json!({ "relativeRepoRoot": normalize_prompt_path(&relative) }))?;
        let rendered = ara_prompt::js::trim(&rendered);
        if !rendered.is_empty() {
            blocks.push(rendered.to_string());
        }
    }
    Ok(SystemPrompt { blocks, warnings })
}
