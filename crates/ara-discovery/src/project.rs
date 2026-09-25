//! Project context assembly (OMP `system-prompt.ts`:
//! `loadProjectContextFiles`, `dedupeContainedContextFiles`).

use crate::at_imports::{Expander, MAX_AT_IMPORT_DEPTH};
use crate::capability::{Capability, CapabilityResult, HostDirs, LoadContext, LoadOptions, ProviderPolicy, SourceMeta};
use crate::context_files::{ContextFile, context_file_capability};
use crate::fs::FsCache;
use std::path::{Path, PathBuf};

/// A context file ready for the system prompt: `@` imports expanded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectContextFile {
    pub path: PathBuf,
    pub content: String,
    pub depth: Option<i64>,
    /// ARA addition: which provider found it and at which level.
    pub source: SourceMeta,
}

/// Host predicate deciding which resolved `@` import targets may be inlined.
pub type ImportPolicy = Box<dyn Fn(&Path) -> bool + Send + Sync>;

/// A host's discovery setup: its directories, provider switches, file
/// cache and registered capabilities.
pub struct Discovery {
    pub home: PathBuf,
    pub dirs: HostDirs,
    pub policy: ProviderPolicy,
    pub fs: FsCache,
    pub context_files: Capability<ContextFile>,
    pub skills: Capability<crate::skills::Skill>,
    /// Host policy for `@` imports (see [`Expander::allow`]).
    pub import_policy: Option<ImportPolicy>,
}

impl Discovery {
    pub fn new(home: &Path, dirs: HostDirs, policy: ProviderPolicy) -> Self {
        let context_files = context_file_capability(&dirs.native_name);
        let skills = crate::skills::skill_capability(&dirs.native_name);
        Discovery {
            home: home.to_path_buf(),
            dirs,
            policy,
            fs: FsCache::new(),
            context_files,
            skills,
            import_policy: None,
        }
    }

    /// A load context rooted at `cwd` (repo root found by walking up to `.git`).
    pub fn context(&self, cwd: &Path) -> LoadContext<'_> {
        let cwd = crate::paths::resolve(cwd);
        LoadContext {
            repo_root: self.fs.find_repo_root(&cwd),
            cwd,
            home: self.home.clone(),
            explicit_providers: None,
            include_opt_out_user_sources: false,
            dirs: &self.dirs,
            policy: &self.policy,
            fs: &self.fs,
            skill_toggles: Default::default(),
        }
    }

    /// `loadCapability("context-files")`.
    pub fn load_context_files(
        &self,
        cwd: &Path,
        options: &LoadOptions<'_, ContextFile>,
    ) -> CapabilityResult<ContextFile> {
        let mut ctx = self.context(cwd);
        if let Some(providers) = &options.providers {
            ctx.explicit_providers = Some(providers.iter().cloned().collect());
        }
        ctx.include_opt_out_user_sources = options.include_disabled;
        self.context_files.load(&ctx, options)
    }

    /// `loadProjectContextFiles`: discovered files with `@` imports expanded,
    /// farther-from-cwd first, files contained in a closer one dropped.
    pub fn load_project_context_files(&self, cwd: &Path, disabled_extensions: &[String]) -> Vec<ProjectContextFile> {
        let options = LoadOptions { disabled_extensions: disabled_extensions.to_vec(), ..LoadOptions::default() };
        let result = self.load_context_files(cwd, &options);
        let allow = self.import_policy.as_deref().map(|f| f as &(dyn Fn(&Path) -> bool + Sync));
        let expander = Expander { fs: &self.fs, home: self.home.clone(), max_depth: MAX_AT_IMPORT_DEPTH, allow };
        let mut files: Vec<ProjectContextFile> = result
            .items
            .into_iter()
            .map(|file| ProjectContextFile {
                content: expander.expand(&file.content, &file.path),
                path: file.path,
                depth: file.depth,
                source: file.source,
            })
            .collect();
        // Depth descending; user files (no depth) count as -1 here.
        files.sort_by(|a, b| b.depth.unwrap_or(-1).cmp(&a.depth.unwrap_or(-1)));
        dedupe_contained_context_files(files)
    }
}

fn first_non_empty(content: &str) -> Option<&str> {
    let trimmed = ara_prompt::js::trim(content);
    (!trimmed.is_empty()).then_some(trimmed)
}

/// Paragraph blocks after post-render formatting; fenced code never splits.
pub fn split_comparable_prompt_blocks(content: &str) -> Vec<String> {
    let Some(normalized) = first_non_empty(content) else { return Vec::new() };
    let rendered = ara_prompt::format(normalized, ara_prompt::FormatOptions::default());
    let rendered = ara_prompt::js::trim(&rendered);
    let mut blocks = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let mut in_fence = false;
    for line in rendered.split('\n') {
        let rest = ara_prompt::js::trim_start(line);
        if rest.starts_with("```") || rest.starts_with("~~~") {
            in_fence = !in_fence;
            current.push(line);
            continue;
        }
        let blank = ara_prompt::js::trim(line).is_empty();
        if !in_fence && blank && current.last().is_some_and(|l| !ara_prompt::js::trim(l).is_empty()) {
            let block = current.join("\n");
            let block = ara_prompt::js::trim(&block);
            if !block.is_empty() {
                blocks.push(block.to_string());
            }
            current.clear();
            continue;
        }
        current.push(line);
    }
    let tail = current.join("\n");
    let tail = ara_prompt::js::trim(&tail);
    if !tail.is_empty() {
        blocks.push(tail.to_string());
    }
    blocks
}

/// `rule` appears as a contiguous run inside `source`.
pub fn prompt_blocks_contain(source: &[String], rule: &[String]) -> bool {
    if source.is_empty() || rule.is_empty() || rule.len() > source.len() {
        return false;
    }
    source.windows(rule.len()).any(|window| window == rule)
}

/// Drop a file whose paragraph sequence a more authoritative (closer to
/// cwd) file contains verbatim. Output is depth-descending (files without a
/// depth first), stable among equal depths.
pub fn dedupe_contained_context_files(mut files: Vec<ProjectContextFile>) -> Vec<ProjectContextFile> {
    let rank = |f: &ProjectContextFile| f.depth.map_or(f64::INFINITY, |d| d as f64);
    files.sort_by(|a, b| rank(b).total_cmp(&rank(a)));
    let blocks: Vec<Vec<String>> = files.iter().map(|f| split_comparable_prompt_blocks(&f.content)).collect();
    files
        .into_iter()
        .enumerate()
        .filter(|(index, _)| {
            !blocks.iter().skip(index + 1).any(|candidate| prompt_blocks_contain(candidate, &blocks[*index]))
        })
        .map(|(_, file)| file)
        .collect()
}
