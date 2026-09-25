//! Capability registry (OMP `capability/index.ts`, `capability/types.ts`):
//! providers sorted by priority, loaded into one list where the first item
//! per key wins, then validated.

use crate::fs::FsCache;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Where an item came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Level {
    User,
    Project,
    Native,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::User => "user",
            Level::Project => "project",
            Level::Native => "native",
        }
    }
}

/// Provenance attached to every loaded item (`SourceMeta`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceMeta {
    /// Provider ID that loaded the item.
    pub provider: String,
    /// Provider display name, filled in by the registry.
    pub provider_name: String,
    /// Absolute path of the source file.
    pub path: PathBuf,
    pub level: Level,
}

impl SourceMeta {
    /// `createSourceMeta` (the display name is set when the registry loads).
    pub fn new(provider: &str, path: &Path, level: Level) -> Self {
        SourceMeta {
            provider: provider.to_string(),
            provider_name: String::new(),
            path: crate::paths::resolve(path),
            level,
        }
    }
}

/// An item a capability collects.
pub trait Sourced {
    fn source(&self) -> &SourceMeta;
    fn source_mut(&mut self) -> &mut SourceMeta;
}

/// Host-owned locations foreign and native config live in. Upstream reads
/// these from process state (`getAgentDir()`, `CLAUDE_CONFIG_DIR`,
/// `COPILOT_HOME`, …); ARA takes them from the host.
#[derive(Clone, Debug)]
pub struct HostDirs {
    /// Display name of the native provider (upstream "OMP").
    pub native_name: String,
    /// Native user config dir (upstream `getAgentDir()`, `~/.omp/agent`).
    pub native_user_dir: PathBuf,
    /// Native project config dir name (upstream `.omp`).
    pub native_project_dir: String,
    /// `CLAUDE_CONFIG_DIR` override for Claude Code's user config dir.
    pub claude_config_dir: Option<PathBuf>,
    /// `COPILOT_HOME` override for GitHub Copilot's user config root.
    pub copilot_home: Option<PathBuf>,
    /// `COPILOT_CUSTOM_INSTRUCTIONS_DIRS`.
    pub copilot_custom_instruction_dirs: Vec<PathBuf>,
    /// Extra user homes searched for `~/.agent[s]` (upstream: the Windows
    /// profile under WSL).
    pub extra_user_homes: Vec<PathBuf>,
}

impl HostDirs {
    /// ARA's native layout: `~/.ara/agent` and `.ara/`.
    pub fn ara(home: &Path) -> Self {
        HostDirs {
            native_name: "ARA".into(),
            native_user_dir: home.join(".ara").join("agent"),
            native_project_dir: ".ara".into(),
            claude_config_dir: None,
            copilot_home: None,
            copilot_custom_instruction_dirs: Vec::new(),
            extra_user_homes: Vec::new(),
        }
    }

    /// Fill the foreign-tool overrides from environment variables the way
    /// upstream reads them: `CLAUDE_CONFIG_DIR`, `COPILOT_HOME`,
    /// `COPILOT_CUSTOM_INSTRUCTIONS_DIRS` (comma separated) and, under WSL
    /// (`WSL_DISTRO_NAME`/`WSL_INTEROP`), the `USERPROFILE` drive path mapped
    /// to `/mnt/<drive>/…`. Upstream also probes `cmd.exe` when
    /// `USERPROFILE` is unset; ARA spawns no process during discovery.
    pub fn with_env(mut self, env: impl Fn(&str) -> Option<String>) -> Self {
        let trimmed = |name: &str| env(name).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        self.claude_config_dir = trimmed("CLAUDE_CONFIG_DIR").map(|dir| crate::paths::resolve(Path::new(&dir)));
        self.copilot_home = trimmed("COPILOT_HOME").map(PathBuf::from);
        self.copilot_custom_instruction_dirs = env("COPILOT_CUSTOM_INSTRUCTIONS_DIRS")
            .map(|raw| raw.split(',').map(str::trim).filter(|s| !s.is_empty()).map(PathBuf::from).collect())
            .unwrap_or_default();
        let wsl = cfg!(target_os = "linux")
            && (env("WSL_DISTRO_NAME").is_some_and(|v| !v.is_empty())
                || env("WSL_INTEROP").is_some_and(|v| !v.is_empty()));
        if wsl && let Some(profile) = env("USERPROFILE").and_then(|p| windows_path_to_wsl_mount(&p)) {
            self.extra_user_homes = vec![profile];
        }
        self
    }
}

/// `windowsPathToWslMount` for plain drive paths (`C:\Users\me`).
fn windows_path_to_wsl_mount(path: &str) -> Option<PathBuf> {
    let path = path.trim();
    if path.starts_with('/') {
        return Some(PathBuf::from(path));
    }
    let mut chars = path.chars();
    let drive = chars.next().filter(char::is_ascii_alphabetic)?;
    if chars.next() != Some(':') {
        return None;
    }
    let rest = &path[2..];
    if !rest.is_empty() && !rest.starts_with(['\\', '/']) {
        return None;
    }
    let mut out = PathBuf::from("/mnt").join(drive.to_ascii_lowercase().to_string());
    for segment in rest.split(['\\', '/']).filter(|s| !s.is_empty() && *s != ".") {
        if segment == ".." {
            out.pop();
        } else {
            out.push(segment);
        }
    }
    Some(out)
}

/// Foreign tools whose user-level (`~/…`) config is opt-in.
const FOREIGN_USER_PROVIDERS: &[&str] =
    &["cursor", "codex", "claude", "claude-plugins", "gemini", "opencode", "windsurf", "github"];

/// Provider switches a host persists (upstream `disabledProviders` /
/// `enabledProviders` settings).
#[derive(Clone, Debug, Default)]
pub struct ProviderPolicy {
    /// Providers switched off entirely.
    pub disabled: HashSet<String>,
    /// Foreign providers whose `~/` config is opted in (`*`/`all` for every one).
    pub enabled_user_sources: HashSet<String>,
}

/// Inputs every provider loader receives (`LoadContext`).
pub struct LoadContext<'a> {
    pub cwd: PathBuf,
    pub home: PathBuf,
    /// Directory holding `.git`, if any.
    pub repo_root: Option<PathBuf>,
    pub explicit_providers: Option<HashSet<String>>,
    /// Scan opted-out foreign `~/` sources too (dashboard-style loads).
    pub include_opt_out_user_sources: bool,
    pub dirs: &'a HostDirs,
    pub policy: &'a ProviderPolicy,
    pub fs: &'a FsCache,
}

impl LoadContext<'_> {
    /// `isUserSourceEnabled`: native and `.agents` user config always load;
    /// foreign tools' `~/` config only when opted in.
    pub fn is_user_source_enabled(&self, source: &str) -> bool {
        let id = source.strip_prefix('.').unwrap_or(source);
        if self.policy.disabled.contains(id) {
            return false;
        }
        if !FOREIGN_USER_PROVIDERS.contains(&id) {
            return true;
        }
        if self.explicit_providers.as_ref().is_some_and(|p| p.contains(id)) || self.include_opt_out_user_sources {
            return true;
        }
        let enabled = &self.policy.enabled_user_sources;
        if enabled.contains(id) || enabled.contains("*") || enabled.contains("all") {
            return true;
        }
        (id == "claude-plugins" && enabled.contains("claude"))
            || (id == "claude" && self.dirs.claude_config_dir.is_some())
    }
}

/// Items and warnings from one provider.
pub struct LoadResult<T> {
    pub items: Vec<T>,
    pub warnings: Vec<String>,
}

impl<T> Default for LoadResult<T> {
    fn default() -> Self {
        LoadResult { items: Vec::new(), warnings: Vec::new() }
    }
}

pub type LoadFn<T> = Arc<dyn Fn(&LoadContext<'_>) -> Result<LoadResult<T>, String> + Send + Sync>;

pub struct Provider<T> {
    pub id: String,
    pub display_name: String,
    pub description: String,
    /// Higher loads first and wins key conflicts.
    pub priority: i32,
    pub load: LoadFn<T>,
}

impl<T> Clone for Provider<T> {
    fn clone(&self) -> Self {
        Provider {
            id: self.id.clone(),
            display_name: self.display_name.clone(),
            description: self.description.clone(),
            priority: self.priority,
            load: Arc::clone(&self.load),
        }
    }
}

/// A kind of discovered resource and how its items deduplicate.
pub struct Capability<T> {
    pub id: String,
    pub display_name: String,
    pub description: String,
    /// Dedupe key; the first item per key wins. `None` never deduplicates.
    pub key: fn(&T) -> Option<String>,
    /// Items with different keys that still alias each other.
    pub equivalent: Option<fn(&T, &T) -> bool>,
    pub validate: Option<fn(&T) -> Option<String>>,
    /// `disabledExtensions` ID.
    pub to_extension_id: Option<fn(&T) -> Option<String>>,
    providers: Vec<Provider<T>>,
}

impl<T> Capability<T> {
    pub fn new(id: &str, display_name: &str, description: &str, key: fn(&T) -> Option<String>) -> Self {
        Capability {
            id: id.into(),
            display_name: display_name.into(),
            description: description.into(),
            key,
            equivalent: None,
            validate: None,
            to_extension_id: None,
            providers: Vec::new(),
        }
    }

    /// Insert by priority, highest first; equal priorities keep registration order.
    pub fn register(&mut self, provider: Provider<T>) {
        let index = self.providers.iter().position(|p| p.priority < provider.priority).unwrap_or(self.providers.len());
        self.providers.insert(index, provider);
    }

    pub fn providers(&self) -> &[Provider<T>] {
        &self.providers
    }
}

/// Load options (`LoadOptions`).
pub struct LoadOptions<'a, T> {
    /// Only these provider IDs.
    pub providers: Option<Vec<String>>,
    pub exclude_providers: Vec<String>,
    pub include_invalid: bool,
    /// Keep items whose extension ID is disabled.
    pub include_disabled: bool,
    pub disabled_extensions: Vec<String>,
    /// Drop items before deduplication (they claim no key).
    pub filter: Option<&'a dyn Fn(&T) -> bool>,
    /// Hide items that still claim their key.
    pub suppress: Option<&'a dyn Fn(&T) -> bool>,
}

impl<T> Default for LoadOptions<'_, T> {
    fn default() -> Self {
        LoadOptions {
            providers: None,
            exclude_providers: Vec::new(),
            include_invalid: false,
            include_disabled: false,
            disabled_extensions: Vec::new(),
            filter: None,
            suppress: None,
        }
    }
}

/// Merged result (`CapabilityResult`).
pub struct CapabilityResult<T> {
    /// Deduplicated, valid items in priority order.
    pub items: Vec<T>,
    /// Every contributed item with its shadowed flag (diagnostics).
    pub all: Vec<(T, bool)>,
    pub warnings: Vec<String>,
    /// Providers that contributed at least one item.
    pub providers: Vec<String>,
}

impl<T: Sourced + Clone> Capability<T> {
    /// `loadCapability` over an existing context.
    pub fn load(&self, ctx: &LoadContext<'_>, options: &LoadOptions<'_, T>) -> CapabilityResult<T> {
        let providers = self.providers.iter().filter(|p| {
            !ctx.policy.disabled.contains(&p.id)
                && options.providers.as_ref().is_none_or(|allowed| allowed.contains(&p.id))
                && !options.exclude_providers.contains(&p.id)
        });
        let disabled: HashSet<&str> = if options.include_disabled {
            HashSet::new()
        } else {
            options.disabled_extensions.iter().map(String::as_str).collect()
        };

        let mut all: Vec<(T, bool, bool)> = Vec::new(); // item, suppressed, shadowed
        let mut warnings = Vec::new();
        let mut contributing = Vec::new();
        for provider in providers {
            let result = match (provider.load)(ctx) {
                Ok(result) => result,
                Err(error) => {
                    warnings.push(format!("[{}] Failed to load: {error}", provider.display_name));
                    continue;
                }
            };
            warnings.extend(result.warnings.iter().map(|w| format!("[{}] {w}", provider.display_name)));
            let mut contributed = 0;
            for mut item in result.items {
                if let Some(id) = self.to_extension_id.and_then(|f| f(&item))
                    && disabled.contains(id.as_str())
                {
                    continue;
                }
                if options.filter.is_some_and(|f| !f(&item)) {
                    continue;
                }
                item.source_mut().provider_name = provider.display_name.clone();
                if options.suppress.is_some_and(|f| f(&item)) {
                    all.push((item, true, false));
                    continue;
                }
                all.push((item, false, false));
                contributed += 1;
            }
            if contributed > 0 {
                contributing.push(provider.id.clone());
            }
        }

        let mut seen = HashSet::new();
        let mut deduped: Vec<T> = Vec::new();
        for (item, suppressed, shadowed) in &mut all {
            let key = (self.key)(item);
            if *suppressed {
                if let Some(key) = key {
                    seen.insert(key);
                }
                continue;
            }
            let Some(key) = key else {
                deduped.push(item.clone());
                continue;
            };
            let key_seen = !seen.insert(key);
            let alias_seen = !key_seen && self.equivalent.is_some_and(|eq| deduped.iter().any(|e| eq(e, item)));
            if key_seen || alias_seen {
                *shadowed = true;
            } else {
                deduped.push(item.clone());
            }
        }

        if let Some(validate) = self.validate.filter(|_| !options.include_invalid) {
            deduped.retain(|item| match validate(item) {
                Some(error) => {
                    let source = item.source();
                    warnings.push(format!(
                        "[{}] Invalid item at {}: {error}",
                        source.provider_name,
                        source.path.display()
                    ));
                    false
                }
                None => true,
            });
        }

        CapabilityResult {
            items: deduped,
            all: all
                .into_iter()
                .filter(|(_, suppressed, _)| !suppressed)
                .map(|(item, _, shadowed)| (item, shadowed))
                .collect(),
            warnings,
            providers: contributing,
        }
    }
}
