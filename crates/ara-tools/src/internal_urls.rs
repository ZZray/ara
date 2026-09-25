//! Internal URL resolution for file tools (OMP `internal-urls/`). Only
//! `skill://` is implemented (`skill-protocol.ts`): `skill://<name>` is the
//! skill's `SKILL.md`, `skill://<name>/<path>` a file inside its directory.

use std::path::{Component, Path, PathBuf};

/// A loaded skill as the tools see it (the host supplies the list).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillRef {
    pub name: String,
    pub file_path: PathBuf,
    pub base_dir: PathBuf,
}

/// Whether `input` uses a scheme these tools resolve.
pub fn is_internal_url(input: &str) -> bool {
    input.starts_with("skill://")
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) =
                ((bytes[i + 1] as char).to_digit(16), bytes.get(i + 2).and_then(|b| (*b as char).to_digit(16)))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Resolve a `skill://` URL to a filesystem path (upstream error texts).
pub fn resolve_skill_url(skills: &[SkillRef], url: &str) -> Result<PathBuf, String> {
    let rest = url.strip_prefix("skill://").unwrap_or(url);
    let (name, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    if name.is_empty() {
        return Err("skill:// URL requires a skill name: skill://<name>".into());
    }
    let Some(skill) = skills.iter().find(|s| s.name == name) else {
        let available = if skills.is_empty() {
            "none".to_string()
        } else {
            skills.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ")
        };
        return Err(format!("Unknown skill: {name}\nAvailable: {available}"));
    };
    if path.is_empty() || path == "/" {
        return Ok(skill.file_path.clone());
    }
    let relative = percent_decode(&path[1..]);
    if Path::new(&relative).is_absolute() {
        return Err("Absolute paths are not allowed in skill:// URLs".into());
    }
    if relative.split(['/', '\\']).any(|part| part == "..")
        || Path::new(&relative).components().any(|c| c == Component::ParentDir)
    {
        return Err("Path traversal (..) is not allowed in skill:// URLs".into());
    }
    let target = skill.base_dir.join(&relative);
    if !target.starts_with(&skill.base_dir) {
        return Err("Path traversal is not allowed".into());
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_urls() {
        let skills = vec![SkillRef {
            name: "demo".into(),
            file_path: PathBuf::from("/s/demo/SKILL.md"),
            base_dir: PathBuf::from("/s/demo"),
        }];
        assert_eq!(resolve_skill_url(&skills, "skill://demo").unwrap(), PathBuf::from("/s/demo/SKILL.md"));
        assert_eq!(resolve_skill_url(&skills, "skill://demo/").unwrap(), PathBuf::from("/s/demo/SKILL.md"));
        assert_eq!(
            resolve_skill_url(&skills, "skill://demo/scripts/a%20b.sh").unwrap(),
            PathBuf::from("/s/demo/scripts/a b.sh")
        );
        assert_eq!(
            resolve_skill_url(&skills, "skill://").unwrap_err(),
            "skill:// URL requires a skill name: skill://<name>"
        );
        assert_eq!(resolve_skill_url(&skills, "skill://nope").unwrap_err(), "Unknown skill: nope\nAvailable: demo");
        assert_eq!(resolve_skill_url(&[], "skill://nope").unwrap_err(), "Unknown skill: nope\nAvailable: none");
        assert_eq!(
            resolve_skill_url(&skills, "skill://demo//etc/passwd").unwrap_err(),
            "Absolute paths are not allowed in skill:// URLs"
        );
        assert_eq!(
            resolve_skill_url(&skills, "skill://demo/../x").unwrap_err(),
            "Path traversal (..) is not allowed in skill:// URLs"
        );
        assert_eq!(
            resolve_skill_url(&skills, "skill://demo/a/%2E%2E/x").unwrap_err(),
            "Path traversal (..) is not allowed in skill:// URLs"
        );
    }
}

// ---------------------------------------------------------------------------
// bash: `skill://` expansion (`tools/bash-skill-urls.ts`, skill scheme)
// ---------------------------------------------------------------------------

/// `resolveSkillUrlToPath`: a bare `skill://name` is the skill's directory;
/// the name is matched longest-prefix across `:` (namespaced skills).
pub fn resolve_skill_url_to_path(skills: &[SkillRef], url: &str) -> Result<PathBuf, String> {
    let Some(rest) = url.strip_prefix("skill://") else { return Err(format!("Invalid skill:// URL: {url}")) };
    let rest = rest.split(['?', '#']).next().unwrap_or_default();
    let (segment, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    if segment.is_empty() {
        return Err(format!("Invalid skill:// URL: {url}"));
    }
    let raw = percent_decode(segment);
    let (skill, suffix) = match skills.iter().find(|s| s.name == raw) {
        Some(skill) => (Some(skill), None),
        None => {
            let mut candidate = raw.as_str();
            let mut found = (None, None);
            while let Some(colon) = candidate.rfind(':').filter(|&i| i > 0) {
                candidate = &candidate[..colon];
                if let Some(skill) = skills.iter().find(|s| s.name == candidate) {
                    found = (Some(skill), Some(raw[colon + 1..].to_string()));
                    break;
                }
            }
            found
        }
    };
    let Some(skill) = skill else {
        let available = if skills.is_empty() {
            "none".to_string()
        } else {
            skills.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ")
        };
        return Err(format!("Unknown skill: {raw}. Available: {available}"));
    };
    let raw_path = format!("{path}{}", suffix.map(|s| format!("/{s}")).unwrap_or_default());
    if raw_path.is_empty() || raw_path == "/" {
        return Ok(skill.base_dir.clone());
    }
    let relative = percent_decode(&raw_path[1..]);
    if Path::new(&relative).is_absolute() {
        return Err("Absolute paths are not allowed in skill:// URLs".into());
    }
    if relative.split(['/', '\\']).any(|part| part == "..") {
        return Err("Path traversal (..) is not allowed in skill:// URLs".into());
    }
    let target = skill.base_dir.join(&relative);
    if !target.starts_with(&skill.base_dir) {
        return Err("Path traversal is not allowed in skill:// URLs".into());
    }
    Ok(target)
}

static SKILL_URL_IN_COMMAND: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    // JS `\s` spelled out; unquoted URLs stop before shell syntax.
    const S: &str = r"\t\n\x0B\x0C\r \u{A0}\u{1680}\u{2000}-\u{200A}\u{2028}\u{2029}\u{202F}\u{205F}\u{3000}\u{FEFF}";
    regex::Regex::new(&format!(r#"'skill://[^'{S}")`\\]+'|"skill://[^"{S}')`\\]+"|skill://[^{S}'")`\;&|<>($]+"#))
        .unwrap()
});

/// Whether `index` lies inside a shell quote (`isInsideShellQuote`), tracking
/// `$(…)` and backtick substitutions.
fn is_inside_shell_quote(command: &str, index: usize) -> bool {
    #[derive(Clone, Copy, PartialEq)]
    enum Kind {
        Dollar,
        Backtick,
    }
    let bytes = command.as_bytes();
    let mut quote: Option<u8> = None;
    let mut subs: Vec<(Kind, Option<u8>, i32)> = Vec::new();
    let mut i = 0;
    while i < index && i < bytes.len() {
        let c = bytes[i];
        let top = subs.last().copied();
        if c == b'\\'
            && bytes.get(i + 1) == Some(&b'"')
            && quote != Some(b'\'')
            && top.is_some_and(|(k, outer, _)| k == Kind::Backtick && outer == Some(b'"'))
        {
            quote = if quote == Some(b'"') { None } else { Some(b'"') };
            i += 2;
            continue;
        }
        if c == b'\\' && quote != Some(b'\'') {
            i += 2;
            continue;
        }
        if c == b'\'' && quote != Some(b'"') {
            quote = if quote == Some(b'\'') { None } else { Some(b'\'') };
        } else if c == b'"' && quote != Some(b'\'') {
            quote = if quote == Some(b'"') { None } else { Some(b'"') };
        } else if c == b'$' && bytes.get(i + 1) == Some(&b'(') && quote != Some(b'\'') {
            subs.push((Kind::Dollar, quote, 1));
            quote = None;
            i += 1;
        } else if c == b'`' && quote != Some(b'\'') {
            if top.is_some_and(|(k, _, _)| k == Kind::Backtick) {
                quote = subs.pop().and_then(|(_, outer, _)| outer);
            } else {
                subs.push((Kind::Backtick, quote, 0));
                quote = None;
            }
        } else if quote.is_none()
            && let Some(last) = subs.last_mut()
            && last.0 == Kind::Dollar
        {
            if c == b'(' {
                last.2 += 1;
            } else if c == b')' {
                last.2 -= 1;
                if last.2 == 0 {
                    quote = subs.pop().and_then(|(_, outer, _)| outer);
                }
            }
        }
        i += 1;
    }
    quote.is_some()
}

fn shell_escape(p: &str) -> String {
    format!("'{}'", p.replace('\'', "'\\''"))
}

/// `expandInternalUrls` for `skill://`: resolvable URLs become (shell-escaped)
/// absolute paths; unresolvable ones and mentions inside larger quoted text
/// stay as written.
pub fn expand_skill_urls(command: &str, skills: &[SkillRef], no_escape: bool) -> String {
    if !command.contains("skill://") {
        return command.to_string();
    }
    let matches: Vec<(usize, String)> =
        SKILL_URL_IN_COMMAND.find_iter(command).map(|m| (m.start(), m.as_str().to_string())).collect();
    let mut expanded = command.to_string();
    for (index, token) in matches.into_iter().rev() {
        let quoted = token.starts_with('\'') || token.starts_with('"');
        if !quoted && is_inside_shell_quote(command, index) {
            continue;
        }
        let url = if quoted { &token[1..token.len() - 1] } else { token.as_str() };
        let Ok(path) = resolve_skill_url_to_path(skills, url) else { continue };
        let path = path.to_string_lossy();
        let replacement = if no_escape { path.into_owned() } else { shell_escape(&path) };
        expanded.replace_range(index..index + token.len(), &replacement);
    }
    expanded
}

#[cfg(test)]
mod bash_tests {
    use super::*;

    fn skills() -> Vec<SkillRef> {
        vec![
            SkillRef { name: "demo".into(), file_path: "/s/demo/SKILL.md".into(), base_dir: "/s/demo".into() },
            SkillRef { name: "pkg:tool".into(), file_path: "/p/tool/SKILL.md".into(), base_dir: "/p/tool".into() },
        ]
    }

    #[test]
    fn expands_skill_urls_in_commands() {
        let s = skills();
        assert_eq!(expand_skill_urls("cat skill://demo/SKILL.md", &s, false), "cat '/s/demo/SKILL.md'");
        assert_eq!(expand_skill_urls("ls skill://demo; echo", &s, false), "ls '/s/demo'; echo");
        assert_eq!(expand_skill_urls("cat 'skill://demo/a.txt'", &s, false), "cat '/s/demo/a.txt'");
        // Quoted tokens exclude whitespace (upstream pattern): left as written.
        assert_eq!(expand_skill_urls("cat 'skill://demo/a b.txt'", &s, false), "cat 'skill://demo/a b.txt'");
        assert_eq!(expand_skill_urls("echo \"see skill://demo here\"", &s, false), "echo \"see skill://demo here\"");
        assert_eq!(expand_skill_urls("cat skill://nope/x", &s, false), "cat skill://nope/x");
        assert_eq!(expand_skill_urls("cat skill://demo/../x", &s, false), "cat skill://demo/../x");
        assert_eq!(expand_skill_urls("cat skill://pkg:tool:run.sh", &s, false), "cat '/p/tool/run.sh'");
        assert_eq!(expand_skill_urls("skill://demo", &s, true), "/s/demo");
        assert_eq!(
            resolve_skill_url_to_path(&s, "skill://nope").unwrap_err(),
            "Unknown skill: nope. Available: demo, pkg:tool"
        );
    }
}
