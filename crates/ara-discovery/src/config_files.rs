//! Named files in the native and foreign config directories (OMP `config.ts`
//! `getConfigDirs` / `findConfigFile`), as `main.ts` uses them to discover
//! `SYSTEM.md` and `APPEND_SYSTEM.md`.

use crate::project::Discovery;
use std::path::{Path, PathBuf};

/// Which config directories to search (`getConfigDirs` `user` / `project`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigLevel {
    User,
    Project,
}

/// Foreign bases after the native one, in upstream priority order.
const FOREIGN_BASES: [&str; 3] = [".claude", ".codex", ".gemini"];

impl Discovery {
    /// `getConfigDirs("", { user, project })` for one level, highest priority
    /// first.
    /// - User: the native user dir (`~/.ara/agent`), then Claude's config
    ///   dir, `~/.codex` and `~/.gemini`, each only when that user source is
    ///   enabled.
    /// - Project: `<cwd>/.ara`, `.claude`, `.codex`, `.gemini`. The project
    ///   dir is `cwd` itself, with no walk up.
    pub fn config_dirs(&self, cwd: &Path, level: ConfigLevel) -> Vec<PathBuf> {
        let cwd = crate::paths::resolve(cwd);
        match level {
            ConfigLevel::User => {
                let ctx = self.context(&cwd);
                let mut dirs = vec![crate::paths::resolve(&self.dirs.native_user_dir)];
                for base in FOREIGN_BASES {
                    if !ctx.is_user_source_enabled(base) {
                        continue;
                    }
                    dirs.push(match base {
                        ".claude" => self.dirs.claude_config_dir.clone().unwrap_or_else(|| self.home.join(".claude")),
                        _ => self.home.join(base),
                    });
                }
                dirs
            }
            ConfigLevel::Project => std::iter::once(self.dirs.native_project_dir.as_str())
                .chain(FOREIGN_BASES)
                .map(|base| cwd.join(base))
                .collect(),
        }
    }

    /// `findConfigFile(name, …)` for one level: the first `<dir>/<name>` that
    /// exists (`fs.existsSync`: any type, symlinks followed).
    pub fn find_config_file(&self, cwd: &Path, name: &str, level: ConfigLevel) -> Option<PathBuf> {
        self.config_dirs(cwd, level).into_iter().map(|dir| dir.join(name)).find(|path| path.exists())
    }

    /// `discoverSystemPromptFile` / `discoverAppendSystemPromptFile`
    /// (`main.ts`): the project file first, then the user file.
    pub fn discover_prompt_file(&self, cwd: &Path, name: &str) -> Option<PathBuf> {
        self.find_config_file(cwd, name, ConfigLevel::Project)
            .or_else(|| self.find_config_file(cwd, name, ConfigLevel::User))
    }
}
