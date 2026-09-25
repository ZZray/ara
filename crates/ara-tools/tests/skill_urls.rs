//! Ports of OMP `packages/coding-agent/test/tools/bash-skill-urls.test.ts`
//! (`expandSkillUrls` and the `skill://` cases of `expandInternalUrls`) at
//! 596f2da7101178214aa27a753529d15e6b7ad91d, plus tool-level checks of
//! `skill://` in `read` and `bash`.
//!
//! Not ported here: the `expandInternalUrls` cases for agent, artifact,
//! memory, rule, local and attachment URLs (those schemes are open).

use ara_agent::{AgentTool, ToolOutput, UpdateFn};
use ara_ai::{JsonObject, UserBlock};
use ara_tools::internal_urls::{SkillRef, expand_skill_urls, expand_skill_urls_strict};
use ara_tools::*;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

fn shell_escape(p: &str) -> String {
    format!("'{}'", p.replace('\'', "'\\''"))
}

fn skill(name: &str, base_dir: &str) -> SkillRef {
    let base = PathBuf::from(base_dir);
    SkillRef { name: name.into(), file_path: base.join("SKILL.md"), base_dir: base }
}

fn joined(skill: &SkillRef, rel: &str) -> String {
    skill.base_dir.join(rel).display().to_string()
}

/// B-70135ff08c, B-75714c16b6, B-215febcd79, B-cb79e03ca6, B-61eb7749db,
/// B-6285e3e688, B-09443b5b01
#[test]
fn expand_skill_urls_to_escaped_paths() {
    let skills = [skill("valid-skill", "/tmp/skills/valid-skill")];
    let expected = shell_escape(&joined(&skills[0], "scripts/init.py"));
    let strict = |c: &str, s: &[SkillRef]| expand_skill_urls_strict(c, s).unwrap();
    assert_eq!(strict("python skill://valid-skill/scripts/init.py", &skills), format!("python {expected}"));
    assert_eq!(strict("python \"skill://valid-skill/scripts/init.py\"", &skills), format!("python {expected}"));
    assert_eq!(strict("python 'skill://valid-skill/scripts/init.py'", &skills), format!("python {expected}"));

    let two = [skill("first-skill", "/tmp/skills/first-skill"), skill("second-skill", "/tmp/skills/second-skill")];
    assert_eq!(
        strict("cp skill://first-skill/a.txt skill://second-skill/b.txt", &two),
        format!("cp {} {}", shell_escape(&joined(&two[0], "a.txt")), shell_escape(&joined(&two[1], "b.txt")))
    );

    let space = [skill("space-skill", "/tmp/skills/with space")];
    assert_eq!(
        strict("python skill://space-skill/scripts/my%20file.py", &space),
        format!("python {}", shell_escape(&joined(&space[0], "scripts/my file.py")))
    );
    let quote = [skill("quote-skill", "/tmp/skills/with'quote")];
    assert_eq!(
        strict("python skill://quote-skill/scripts/init.py", &quote),
        format!("python {}", shell_escape(&joined(&quote[0], "scripts/init.py")))
    );
    assert_eq!(
        strict("printf '%s\n' skill://valid-skill", &skills),
        format!("printf '%s\n' {}", shell_escape("/tmp/skills/valid-skill"))
    );
}

/// B-7298262e3e, B-4121e4ea79, B-841a488914
#[test]
fn expand_skill_urls_errors() {
    let two = [skill("first-skill", "/tmp/skills/first-skill"), skill("second-skill", "/tmp/skills/second-skill")];
    assert_eq!(
        expand_skill_urls_strict("python skill://missing/run.py", &two).unwrap_err(),
        "Unknown skill: missing. Available: first-skill, second-skill"
    );
    let skills = [skill("valid-skill", "/tmp/skills/valid-skill")];
    let traversal = "Path traversal (..) is not allowed in skill:// URLs";
    assert_eq!(
        expand_skill_urls_strict("cat skill://valid-skill/../../../etc/passwd", &skills).unwrap_err(),
        traversal
    );
    assert_eq!(
        expand_skill_urls_strict("cat skill://valid-skill/%2E%2E/%2E%2E/etc/passwd", &skills).unwrap_err(),
        traversal
    );
    // The lenient expansion bash uses leaves the same tokens as written.
    for command in ["python skill://missing/run.py", "cat skill://valid-skill/%2E%2E/%2E%2E/etc/passwd"] {
        assert_eq!(expand_skill_urls(command, &skills, false), command);
    }
}

/// B-b0f1002440, B-1718d266ac, B-62088f2520
#[test]
fn expand_skill_urls_leaves_other_commands_unchanged() {
    let skills = [skill("valid-skill", "/tmp/skills/valid-skill")];
    for command in ["git status", "echo agent://1 artifact://abc rule://security"] {
        assert_eq!(expand_skill_urls_strict(command, &skills).unwrap(), command);
        assert_eq!(expand_skill_urls(command, &skills, false), command);
    }
    let command = "python skill://valid-skill/scripts/init.py";
    assert_eq!(expand_skill_urls_strict(command, &[]).unwrap(), command);
    assert_eq!(expand_skill_urls(command, &[], false), command);
}

/// B-82d53fbcc6, B-34a08eb2b0, B-f8702f4e7f, B-46136772fb, B-a84d31e6d1,
/// B-69f586f734, B-255930f324, B-67a7e0032f
#[test]
fn expand_internal_urls_shell_quoting() {
    let skills = [skill("valid-skill", "/tmp/skills/valid-skill")];
    let path = shell_escape(&joined(&skills[0], "SKILL.md"));
    let expand = |c: &str| expand_skill_urls(c, &skills, false);
    assert_eq!(
        expand("echo \"$(realpath skill://valid-skill/SKILL.md 2>&1)\""),
        format!("echo \"$(realpath {path} 2>&1)\"")
    );
    assert_eq!(expand("echo \"`cat skill://valid-skill/SKILL.md`\""), format!("echo \"`cat {path}`\""));
    assert_eq!(expand("echo `cat skill://valid-skill/SKILL.md`"), format!("echo `cat {path}`"));
    assert_eq!(expand("echo \"`echo $(cat skill://valid-skill/SKILL.md)`\""), format!("echo \"`echo $(cat {path})`\""));
    assert_eq!(expand("echo \"$(echo `cat skill://valid-skill/SKILL.md`)\""), format!("echo \"$(echo `cat {path}`)\""));
    for literal in [
        "echo '`cat skill://valid-skill/SKILL.md`'",
        "echo \"\\`skill://valid-skill/SKILL.md\\`\"",
        "echo \"`printf %s \\\"literal skill://valid-skill/SKILL.md\\\"`\"",
    ] {
        assert_eq!(expand(literal), literal);
    }
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

fn args(v: Value) -> JsonObject {
    v.as_object().unwrap().clone()
}

fn noop() -> UpdateFn {
    Arc::new(|_| {})
}

fn text(o: &ToolOutput) -> String {
    o.content
        .iter()
        .filter_map(|b| if let UserBlock::Text(t) = b { Some(t.text.clone()) } else { None })
        .collect::<Vec<_>>()
        .join("\n")
}

fn write_skill(root: &Path, name: &str, body: &str) -> SkillRef {
    let dir = root.join(name);
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    std::fs::write(dir.join("SKILL.md"), format!("---\nname: {name}\ndescription: {name} skill.\n---\n{body}"))
        .unwrap();
    std::fs::write(dir.join("scripts/hello.sh"), "echo hello-from-skill\n").unwrap();
    SkillRef { name: name.into(), file_path: dir.join("SKILL.md"), base_dir: dir }
}

/// `read` of a skill resource: bare URL is SKILL.md, selectors apply, the
/// resource is immutable (no hashline tag even in hashline mode) and exempt
/// from result limits; unknown skills and traversal fail.
#[tokio::test]
async fn read_skill_urls() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let long_body: String = (1..=4000).map(|i| format!("body-line-{i}\n")).collect();
    let demo = write_skill(&root.join("skills"), "demo", &long_body);
    let ctx = ToolContext::new(&root).with_edit(pi_edit::EditMode::Hashline, true).with_skills(vec![demo.clone()]);
    let read = read::ReadTool { ctx };
    let r = |p: &str| read.execute("c", args(json!({"path": p})), CancellationToken::new(), noop());

    // Immutable: no hashline tag, so no numbering unless `readLineNumbers`
    // (upstream `lineNumbers = hashLines || readLineNumbers`).
    let full = r("skill://demo").await.unwrap();
    let body = text(&full);
    assert!(body.starts_with("---\nname: demo\n"), "{}", &body[..body.len().min(200)]);
    assert!(body.ends_with("body-line-4000"), "no 3000-line truncation for skill reads");
    assert!(!body.contains("more lines in file"));
    assert_eq!(full.details.as_ref().unwrap()["resolvedPath"], json!(demo.file_path.display().to_string()));

    assert_eq!(text(&r("skill://demo:-2").await.unwrap()), "body-line-3999\nbody-line-4000");
    assert_eq!(text(&r("skill://demo/scripts/hello.sh").await.unwrap()), "echo hello-from-skill");
    assert_eq!(text(&r("skill://demo/scripts/hello.sh:raw").await.unwrap()), "echo hello-from-skill");
    let mut numbered_ctx = read.ctx.clone();
    numbered_ctx.line_numbers = true;
    let numbered = read::ReadTool { ctx: numbered_ctx };
    let out = numbered.execute("c", args(json!({"path": "skill://demo:2-3"})), CancellationToken::new(), noop());
    assert_eq!(
        text(&out.await.unwrap()),
        "2|name: demo\n3|description: demo skill.\n\n[4001 more lines in file. Use :4 to continue]"
    );

    let err = |p: &'static str| async move { r(p).await.unwrap_err().0 };
    assert_eq!(err("skill://nope").await, "Unknown skill: nope\nAvailable: demo");
    assert_eq!(err("skill://demo/../x").await, "Path traversal (..) is not allowed in skill:// URLs");
    assert!(err("skill://demo/missing.md").await.starts_with("File not found:"));

    // An ordinary file keeps its limits and hashline tag.
    std::fs::write(root.join("plain.txt"), &long_body).unwrap();
    let plain = text(&r("plain.txt").await.unwrap());
    assert!(plain.starts_with("[plain.txt#") && plain.contains("more lines in file"));
}

/// `bash`: `skill://` in the command, env values and cwd resolve to paths.
#[tokio::test]
async fn bash_expands_skill_urls() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let demo = write_skill(&root.join("my skills"), "demo", "body\n");
    let ctx = ToolContext::new(&root).with_edit(pi_edit::EditMode::Hashline, false).with_skills(vec![demo.clone()]);
    let bash = bash::BashTool { ctx };
    let b = |v: Value| bash.execute("c", args(v), CancellationToken::new(), noop());

    let out = b(json!({"command": "sh skill://demo/scripts/hello.sh"})).await.unwrap();
    assert!(text(&out).contains("hello-from-skill"), "{}", text(&out));
    let out = b(json!({"command": "echo \"$SKILL_DIR\"", "env": {"SKILL_DIR": "skill://demo"}})).await.unwrap();
    assert!(text(&out).contains(&demo.base_dir.display().to_string()), "{}", text(&out));
    let out = b(json!({"command": "pwd", "cwd": "skill://demo/scripts"})).await.unwrap();
    assert!(text(&out).contains(&demo.base_dir.join("scripts").display().to_string()), "{}", text(&out));
    // Unknown skills stay literal, so the shell sees the original token.
    let out = b(json!({"command": "echo skill://nope/x"})).await.unwrap();
    assert!(text(&out).contains("skill://nope/x"), "{}", text(&out));
}
