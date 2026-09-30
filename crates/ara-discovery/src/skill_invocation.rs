//! Explicit Skill parsing, ported from OMP `extensibility/skills.ts:409-487`
//! at 596f2da7101178214aa27a753529d15e6b7ad91d (MIT, Stencil Labs, Inc.).

use ara_prompt::js;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedSkillInvocation {
    pub name: String,
    pub args: String,
}

/// Match fixed OMP's leading command or the first standalone embedded token.
/// The leading form deliberately splits only at an ASCII space.
pub fn parse_skill_invocation(text: &str) -> Option<ParsedSkillInvocation> {
    let start = js::trim_start(text);
    if let Some(command) = start.strip_prefix("/skill:") {
        let (name, args) = command.split_once(' ').map_or((command, ""), |(name, args)| (name, js::trim(args)));
        return (!name.is_empty()).then(|| ParsedSkillInvocation { name: name.into(), args: args.into() });
    }
    if start.starts_with('/') || local_execution_prefix(start) {
        return None;
    }
    for (offset, _) in text.match_indices("/skill:") {
        if offset != 0 && !text[..offset].chars().next_back().is_some_and(js::is_space) {
            continue;
        }
        let name_start = offset + "/skill:".len();
        let rest = &text[name_start..];
        let name_end = rest.find(|c: char| c == '/' || js::is_space(c)).unwrap_or(rest.len());
        let name = &rest[..name_end];
        if name.is_empty() || rest[name_end..].starts_with('/') {
            continue;
        }
        let before = js::trim_end(&text[..offset]);
        let after = js::trim_start(&rest[name_end..]);
        let args = if before.is_empty() {
            after.to_owned()
        } else if after.is_empty() {
            before.to_owned()
        } else {
            format!("{before} {after}")
        };
        return Some(ParsedSkillInvocation { name: name.into(), args: js::trim(&args).into() });
    }
    None
}

fn local_execution_prefix(text: &str) -> bool {
    if text.starts_with('!') {
        return true;
    }
    let Some(rest) = text.strip_prefix('$') else { return false };
    if rest.starts_with('{') {
        return false;
    }
    let rest = rest.strip_prefix('$').unwrap_or(rest);
    rest.is_empty() || rest.starts_with([' ', '\t', '\n', '\r'])
}
