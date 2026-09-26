//! Resource discovery for the ARA Agent Core: the capability registry and
//! the context-file capability (AGENTS.md, CLAUDE.md, GEMINI.md, Copilot
//! instructions) with `@` imports and containment dedupe. Ported from OMP
//! `packages/coding-agent/src/{capability,discovery}` and
//! `system-prompt.ts` at 596f2da7101178214aa27a753529d15e6b7ad91d.
//!
//! Hosts own every location: native config dirs, foreign-tool overrides and
//! provider switches arrive through [`HostDirs`] and [`ProviderPolicy`].

pub mod at_imports;
pub mod capability;
pub mod config_files;
pub mod context_files;
pub mod frontmatter;
pub mod fs;
pub mod paths;
pub mod project;
pub mod skills;
pub mod system_md;

pub use at_imports::{Expander, MAX_AT_IMPORT_DEPTH};
pub use capability::{
    Capability, CapabilityResult, HostDirs, Level, LoadContext, LoadOptions, LoadResult, Provider, ProviderPolicy,
    SkillToggles, SourceMeta, Sourced,
};
pub use context_files::{ContextFile, context_file_capability, load_standalone_context_files};
pub use fs::FsCache;
pub use project::{Discovery, ProjectContextFile, dedupe_contained_context_files};
pub use skills::{LoadedSkill, Skill, SkillWarning, SkillsSettings, skill_capability};
