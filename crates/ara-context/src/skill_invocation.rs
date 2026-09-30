//! User invocation builder from OMP `extensibility/skills.ts:492-531`
//! at 596f2da7101178214aa27a753529d15e6b7ad91d (MIT, Stencil Labs, Inc.).

use ara_discovery::{FsCache, LoadedSkill};
use ara_prompt::js;
use serde_json::{Value, json};
use std::io;

const USER_INVOCATION: &str = include_str!("../prompts/skills/user-invocation.md");

#[derive(Clone, Debug, PartialEq)]
pub struct BuiltSkillPrompt {
    pub message: String,
    pub details: Value,
}

/// Reread only the registered Skill. Async hosts should call this off their
/// runtime worker; discovery's cached/normalized body is not invocation text.
pub fn build_skill_prompt(skill: &LoadedSkill, args: &str) -> io::Result<BuiltSkillPrompt> {
    let content = FsCache::new().read_file_fresh(&skill.file_path)?;
    // Fixed OMP's /^---\n[\s\S]*?\n---\n/ does not strip CRLF headers.
    let body = content
        .strip_prefix("---\n")
        .and_then(|rest| rest.find("\n---\n").map(|end| &rest[end + "\n---\n".len()..]))
        .unwrap_or(&content);
    let body = js::trim(body);
    let args = js::trim(args);
    let message = ara_prompt::render(
        USER_INVOCATION,
        &json!({"name":skill.name, "body":body, "baseDir":skill.base_dir.to_string_lossy(),
            "userArgs":(!args.is_empty()).then_some(args)}),
    )
    .map_err(io::Error::other)?;
    let mut details = json!({"name":skill.name, "path":skill.file_path.to_string_lossy(),
        "lineCount":if body.is_empty() {0} else {body.split('\n').count()}});
    if !args.is_empty() {
        details["args"] = json!(args);
    }
    Ok(BuiltSkillPrompt { message: js::trim(&message).into(), details })
}
