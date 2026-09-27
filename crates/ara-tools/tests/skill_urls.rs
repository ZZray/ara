//! Ports of OMP `packages/coding-agent/test/tools/bash-skill-urls.test.ts`
//! (`expandSkillUrls` and the `skill://` cases of `expandInternalUrls`) at
//! 596f2da7101178214aa27a753529d15e6b7ad91d, plus tool-level checks of
//! `skill://` in `read`, `bash`, `grep`, `glob`, and `write`.
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
    #[cfg(windows)]
    let p = p.replace('\\', "/");
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

/// Search tools use the backing directory for a bare URL. Selectors filter
/// matches, and immutable skill matches never become hashline edit anchors.
#[tokio::test]
async fn grep_skill_urls() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let demo = write_skill(&root.join("skills"), "demo", "needle-in-skill\n");
    std::fs::write(root.join("ordinary.txt"), "needle-in-ordinary\n").unwrap();
    let ctx = ToolContext::new(&root).with_edit(pi_edit::EditMode::Hashline, true).with_skills(vec![demo]);
    let grep = grep::GrepTool::new(ctx);
    let search = |path: &str| {
        grep.execute(
            "c",
            args(json!({"pattern": "needle", "path": path, "gitignore": false})),
            CancellationToken::new(),
            noop(),
        )
    };
    let result = text(&search("skill://demo").await.unwrap());
    assert!(result.contains("needle-in-skill"), "{result}");
    assert!(!result.contains("needle-in-ordinary"), "{result}");
    assert!(!result.contains("SKILL.md#"), "{result}");
    assert!(result.contains("|"), "immutable matches should use plain line numbers: {result}");
    #[cfg(windows)]
    assert!(!result.contains(":/") && !result.contains("//?/"), "workspace files should be relative: {result}");

    let result = text(&search("skill://demo/SKILL.md:5-5:raw").await.unwrap());
    assert!(result.contains("needle-in-skill"), "{result}");
    let result = text(&search("skill://demo/SKILL.md:2-3").await.unwrap());
    assert_eq!(result, "No matches found");
    let mixed = text(&search("skill://demo/SKILL.md; ordinary.txt").await.unwrap());
    assert!(mixed.contains("needle-in-skill") && mixed.contains("needle-in-ordinary"), "{mixed}");
    assert!(!mixed.contains("SKILL.md#"), "{mixed}");
    assert!(mixed.contains("ordinary.txt#"), "{mixed}");
    #[cfg(windows)]
    assert!(!mixed.contains(":/") && !mixed.contains("//?/"), "mixed workspace files should be relative: {mixed}");

    assert!(search("skill://demo/**/*.md").await.unwrap_err().0.contains("Glob patterns are not supported"));
    assert!(search("skill://demo/SKILL.md:raw:conflicts").await.unwrap_err().0.contains("invalid selector"));
    assert!(search("skill://demo/SKILL.md:-10").await.unwrap_err().0.contains("invalid selector"));
    assert!(search("skill://demo/../outside").await.unwrap_err().0.contains("Path traversal"));
    assert!(search("skill://missing").await.unwrap_err().0.contains("Unknown skill"));
    assert!(search("skill://demo/missing.md; ordinary.txt").await.unwrap_err().0.contains("File not found"));
    assert!(text(&search("SKILL://demo/SKILL.md").await.unwrap()).contains("needle-in-skill"));
}

#[tokio::test]
async fn glob_skill_urls() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let demo = write_skill(&root.join("skills"), "demo", "body\n");
    std::fs::write(root.join("ordinary.txt"), "body\n").unwrap();
    let glob = glob::GlobTool::new(ToolContext::new(&root).with_skills(vec![demo]));
    let find = |path: &str| {
        glob.execute("c", args(json!({"path": path, "gitignore": false})), CancellationToken::new(), noop())
    };
    let result = text(&find("skill://demo").await.unwrap());
    assert!(result.contains("SKILL.md") && result.contains("scripts"), "{result}");
    assert!(!result.contains("ordinary.txt"), "{result}");
    #[cfg(windows)]
    assert!(!result.contains(":/") && !result.contains("//?/"), "workspace files should be relative: {result}");
    let result = text(&find("skill://demo/scripts/hello.sh").await.unwrap());
    assert!(result.contains("hello.sh"), "{result}");
    assert!(find("skill://demo/**/*.sh").await.unwrap_err().0.contains("Glob patterns are not supported"));
    assert!(find("skill://demo/../outside").await.unwrap_err().0.contains("Path traversal"));
    assert!(find("skill://demo/missing.md; ordinary.txt").await.unwrap_err().0.contains("File not found"));
    assert!(text(&find("SKILL://demo/SKILL.md").await.unwrap()).contains("SKILL.md"));
}

#[tokio::test]
async fn write_skill_urls_are_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let demo = write_skill(&root.join("skills"), "demo", "original\n");
    let write = write::WriteTool { ctx: ToolContext::new(&root).with_skills(vec![demo.clone()]) };
    for path in ["skill://demo", "skill://demo:raw", "skill://demo/scripts/new.txt", "SKILL://demo/SKILL.md"] {
        let err = write
            .execute("c", args(json!({"path": path, "content": "replacement"})), CancellationToken::new(), noop())
            .await
            .unwrap_err()
            .0;
        assert!(err.contains("read-only for write"), "{path}: {err}");
    }
    let err = write
        .execute(
            "c",
            args(json!({"path": "skill://demo/SKILL.md:1-2", "content": "replacement"})),
            CancellationToken::new(),
            noop(),
        )
        .await
        .unwrap_err()
        .0;
    assert!(err.contains("does not accept the trailing selector"), "{err}");
    assert!(std::fs::read_to_string(&demo.file_path).unwrap().contains("original"));
    assert!(!demo.base_dir.join("scripts/new.txt").exists());
    assert!(!root.join("skill:").exists());
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

    assert_eq!(text(&r("skill://demo:-2").await.unwrap()), "body-line-3998\nbody-line-3999\nbody-line-4000");
    assert_eq!(text(&r("skill://demo/scripts/hello.sh").await.unwrap()), "echo hello-from-skill");
    assert_eq!(text(&r("skill://demo/scripts/hello.sh:raw").await.unwrap()), "echo hello-from-skill\n");
    let mut numbered_ctx = read.ctx.clone();
    numbered_ctx.line_numbers = true;
    let numbered = read::ReadTool { ctx: numbered_ctx };
    let out = numbered.execute("c", args(json!({"path": "skill://demo:2-3"})), CancellationToken::new(), noop());
    assert_eq!(
        text(&out.await.unwrap()),
        "1|---\n2|name: demo\n3|description: demo skill.\n4|---\n5|body-line-1\n6|body-line-2\n\n[3998 more lines in resource. Use :7 to continue]"
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

#[tokio::test]
async fn read_skill_range_includes_context_but_raw_stays_exact() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let demo = write_skill(&root.join("skills"), "demo", "one\ntwo\nthree\nfour\nfive\nsix\nseven\n");
    let references = demo.base_dir.join("references");
    std::fs::create_dir_all(&references).unwrap();
    for line in 1..=8 {
        std::fs::write(references.join(format!("file-{line}.txt")), b"x").unwrap();
    }
    let mut ctx = ToolContext::new(&root).with_skills(vec![demo]);
    ctx.line_numbers = true;
    let read = read::ReadTool { ctx };
    let r = |path: &str| {
        let read = &read;
        let path = path.to_owned();
        async move { read.execute("c", args(json!({"path":path})), CancellationToken::new(), noop()).await.unwrap() }
    };
    assert_eq!(
        text(&r("skill://demo:6-6").await),
        "5|one\n6|two\n7|three\n8|four\n9|five\n\n[2 more lines in resource. Use :10 to continue]"
    );
    let raw = text(&r("skill://demo:raw:6-6").await);
    assert!(raw.starts_with("two\n\n[6 more lines in resource. Use :7 to continue]"), "{raw}");
    assert!(!raw.contains("one") && !raw.contains("three"), "{raw}");
    assert_eq!(
        text(&r("skill://demo/references:4-4").await),
        "3|file-3.txt\n4|file-4.txt\n5|file-5.txt\n6|file-6.txt\n7|file-7.txt\n\n[1 more lines in resource. Use :8 to continue]"
    );
}

/// Fixed OMP resolves skill files as text resources and directory subpaths as
/// immutable, uncapped text listings, even for image and binary extensions.
#[tokio::test]
async fn read_skill_resource_types() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let demo = write_skill(&root.join("skills"), "demo", "body\n");
    std::fs::write(demo.base_dir.join("image.png"), [0x89, b'P', b'N', b'G', 0, b'X']).unwrap();
    std::fs::write(demo.base_dir.join("bytes.bin"), [0xff, 0, b'A', b'\n']).unwrap();
    std::fs::write(demo.base_dir.join("empty.txt"), b"").unwrap();
    std::fs::write(demo.base_dir.join("lines.txt"), b"one\r\ntwo\r\n").unwrap();
    let refs = demo.base_dir.join("references");
    std::fs::create_dir_all(refs.join("zdir")).unwrap();
    for i in 0..=500 {
        std::fs::write(refs.join(format!("file-{i:03}.txt")), b"x").unwrap();
    }
    let read = read::ReadTool { ctx: ToolContext::new(&root).with_skills(vec![demo]) };
    let r = |p: &str| read.execute("c", args(json!({"path": p})), CancellationToken::new(), noop());

    let image = r("skill://demo/image.png").await.unwrap();
    assert_eq!(image.content.len(), 1, "skill images are text resources");
    assert!(text(&image).contains("�PNG\0X"), "{}", text(&image));
    let binary = text(&r("skill://demo/bytes.bin").await.unwrap());
    assert!(binary.contains("�\0A"), "{binary:?}");
    assert!(!binary.contains("Cannot read binary file"), "{binary:?}");
    assert_eq!(text(&r("skill://demo/lines.txt").await.unwrap()), "one\r\ntwo\r");
    let raw = r("skill://demo/lines.txt:raw").await.unwrap();
    assert_eq!(text(&raw), "one\r\ntwo\r\n");
    assert_eq!(raw.details.as_ref().unwrap()["totalLines"], 3);
    assert_eq!(
        text(&r("skill://demo/empty.txt").await.unwrap()),
        "Line 1 is beyond end of resource (0 lines total). The resource is empty."
    );
    let empty_raw = r("skill://demo/empty.txt:raw").await.unwrap();
    assert_eq!(text(&empty_raw), "");
    assert_eq!(empty_raw.details.as_ref().unwrap()["totalLines"], 1);

    let listing = text(&r("skill://demo/references:raw").await.unwrap());
    assert!(listing.starts_with("zdir/\nfile-000.txt"), "{}", &listing[..listing.len().min(100)]);
    assert!(listing.contains("file-500.txt"), "skill directories have no 500-entry cap");
    assert!(!listing.contains("more entries in listing"));
    assert_eq!(
        text(&r("skill://demo/references:raw:1-1").await.unwrap()),
        "zdir/\n\n[501 more lines in resource. Use :2 to continue]"
    );
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
    assert!(text(&out).contains(&demo.base_dir.display().to_string().replace('\\', "/")), "{}", text(&out));
    let out = b(json!({"command": "sh \"$SKILL_DIR/scripts/hello.sh\"", "env": {"SKILL_DIR": "skill://demo"}}))
        .await
        .unwrap();
    assert_eq!(text(&out), "hello-from-skill");
    let out = b(json!({"command": "pwd", "cwd": "skill://demo/scripts"})).await.unwrap();
    let expected = std::process::Command::new("bash")
        .arg("-c")
        .arg("pwd")
        .current_dir(demo.base_dir.join("scripts"))
        .output()
        .unwrap();
    assert!(expected.status.success());
    assert_eq!(text(&out), String::from_utf8_lossy(&expected.stdout).trim());
    // Unknown skills stay literal, so the shell sees the original token.
    let out = b(json!({"command": "echo skill://nope/x"})).await.unwrap();
    assert!(text(&out).contains("skill://nope/x"), "{}", text(&out));
}

/// Review F2: a backslash ends an unquoted token (upstream `\\` in the
/// class), so an escaped quote next to a URL keeps its meaning.
#[test]
fn backslash_ends_an_unquoted_token() {
    let skills = [skill("demo", "/s/demo")];
    let a = shell_escape(&joined(&skills[0], "a"));
    assert_eq!(expand_skill_urls("cat skill://demo/a\\ b", &skills, false), format!("cat {a}\\ b"));
    assert_eq!(expand_skill_urls_strict("cat skill://demo/a\\ b", &skills).unwrap(), format!("cat {a}\\ b"));
    assert_eq!(
        expand_skill_urls("echo skill://demo/x\\\" '\" ; touch M ; \"' skill://demo/y\\\"", &skills, false),
        format!(
            "echo {}\\\" '\" ; touch M ; \"' {}\\\"",
            shell_escape(&joined(&skills[0], "x")),
            shell_escape(&joined(&skills[0], "y"))
        )
    );
}

/// Review F3 (intentional difference): a quoted token whose opening quote is
/// literal text inside another quote is not expanded.
#[test]
fn quoted_tokens_nested_in_quotes_stay_literal() {
    let skills = [skill("demo", "/s/demo")];
    for literal in
        ["echo '\"skill://demo/x;touch${IFS}M\"'", "echo \"'skill://demo/x'\"", "echo \"see \"skill://demo\" here\""]
    {
        assert_eq!(expand_skill_urls(literal, &skills, false), literal);
    }
    // A quote that really opens a string still expands.
    assert_eq!(
        expand_skill_urls("cat \"skill://demo/a\" 'x'", &skills, false),
        format!("cat {} 'x'", shell_escape(&joined(&skills[0], "a")))
    );
}

/// Review F2/F3 through `bash`: text the model quoted never runs, with or
/// without skills loaded.
#[tokio::test]
async fn bash_expansion_never_turns_quoted_text_into_commands() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let demo = write_skill(&root.join("skills"), "demo", "body\n");
    let commands =
        ["echo skill://demo/x\\\" '\" ; touch F2 ; \"' skill://demo/y\\\"", "echo '\"skill://demo/x;touch${IFS}F3\"'"];
    for skills in [Vec::new(), vec![demo.clone()]] {
        let loaded = !skills.is_empty();
        let ctx = ToolContext::new(&root).with_edit(pi_edit::EditMode::Hashline, false).with_skills(skills);
        let bash = bash::BashTool { ctx };
        for command in commands {
            let _ = bash.execute("c", args(json!({"command": command})), CancellationToken::new(), noop()).await;
        }
        for marker in ["F2", "F3"] {
            assert!(!root.join(marker).exists(), "{marker} ran (skills loaded: {loaded})");
        }
    }
}

/// Review F6: `with_skills` gives the context its own list; an earlier clone
/// keeps resolving against its skills.
#[test]
fn with_skills_does_not_touch_earlier_clones() {
    let a = ToolContext::new("/tmp").with_skills(vec![skill("only-a", "/s/a")]);
    let b = a.clone().with_skills(vec![skill("only-b", "/s/b")]);
    assert_eq!(a.resolve_internal_url("skill://only-a").unwrap(), PathBuf::from("/s/a/SKILL.md"));
    assert!(a.resolve_internal_url("skill://only-b").is_err());
    assert_eq!(b.resolve_internal_url("skill://only-b").unwrap(), PathBuf::from("/s/b/SKILL.md"));
    assert!(b.resolve_internal_url("skill://only-a").is_err());
}
