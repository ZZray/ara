//! Ports of OMP coding-agent tests at 596f2da7101178214aa27a753529d15e6b7ad91d:
//! `test/system-prompt-{dedup,personality,model,kernel,inventory}.test.ts` and
//! `test/date-cwd-reminder.test.ts` (unit cases). Each test names its
//! inventory behavior IDs.

use ara_ai::{AssistantMessage, Context, Message, UserBlock, UserContent, UserMessage};
use ara_context::{
    DateCwdReminder, Personality, PromptTool, SystemPromptOptions, build_system_prompt, kernel_identity,
    render_date_cwd_reminder, resolve_prompt_input,
};
use ara_discovery::{Discovery, HostDirs, Level, LoadedSkill, ProjectContextFile, ProviderPolicy, SourceMeta};
use std::fs;
use std::path::{Path, PathBuf};

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
}

fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let home = root.join("home");
    fs::create_dir_all(&home).unwrap();
    Env { _dir: dir, root, home }
}

fn discovery(env: &Env) -> Discovery {
    Discovery::new(&env.home, HostDirs::ara(&env.home), ProviderPolicy::default())
}

fn read_tool() -> Vec<PromptTool> {
    vec![PromptTool { name: "read".into(), label: "Read".into() }]
}

/// Explicit inputs, as upstream tests pass `contextFiles: []`, `skills: []`.
fn explicit() -> SystemPromptOptions {
    SystemPromptOptions {
        context_files: Some(Vec::new()),
        skills: Some(Vec::new()),
        tools: Some(Vec::new()),
        active_repo_context: Some(None),
        ..SystemPromptOptions::default()
    }
}

fn skill(name: &str, description: &str, hide: bool) -> LoadedSkill {
    let path = PathBuf::from(format!("skills/{name}/SKILL.md"));
    LoadedSkill {
        name: name.into(),
        description: description.into(),
        base_dir: path.parent().unwrap().to_path_buf(),
        source: "test".into(),
        hide,
        meta: SourceMeta::new("test", &path, Level::User),
        file_path: path,
    }
}

fn ctx_file(path: &Path, content: &str, depth: i64) -> ProjectContextFile {
    ProjectContextFile {
        path: path.to_path_buf(),
        content: content.into(),
        depth: Some(depth),
        source: SourceMeta::new("test", path, Level::Project),
    }
}

fn render_text(env: &Env, cwd: &Path, options: &SystemPromptOptions) -> (Vec<String>, String) {
    let blocks = build_system_prompt(&discovery(env), cwd, options).unwrap();
    let joined = blocks.join("\n\n");
    (blocks, joined)
}

// --- system-prompt-dedup.test.ts ---------------------------------------------------

/// B-26cbc32b54
#[test]
fn date_and_cwd_stay_out_of_the_system_prompt() {
    let env = env();
    let project = env.home.join("project");
    fs::create_dir_all(&project).unwrap();
    let (_, text) = render_text(&env, &project, &explicit());
    assert!(!text.contains(&project.display().to_string()));
    assert!(!text.contains("Today") && !text.contains("current working directory"));
}

/// B-50437e5125
#[test]
fn system_md_used_as_custom_prompt_renders_once() {
    let env = env();
    let project = env.root.join("project");
    let system = "You are the project SYSTEM prompt.";
    fs::create_dir_all(project.join(".ara")).unwrap();
    fs::write(project.join(".ara/SYSTEM.md"), system).unwrap();
    let options = SystemPromptOptions {
        custom_prompt: Some(system.into()),
        skills: Some(vec![skill("focused-work", "Focused work instructions", false)]),
        tools: Some(read_tool()),
        ..explicit()
    };
    let (_, text) = render_text(&env, &project, &options);
    assert_eq!(text.matches(system).count(), 1);
    assert!(text.contains("<skill name=\"focused-work\">"));
}

/// B-2ecf554176
#[test]
fn loaded_prompt_text_is_not_resolved_as_a_path() {
    let env = env();
    let project = env.root.join("project");
    fs::create_dir_all(&project).unwrap();
    let readme = project.join("README.md");
    fs::write(&readme, "File content that must not replace the prompt.").unwrap();
    let as_text = readme.display().to_string();
    let options = SystemPromptOptions {
        custom_prompt: Some(as_text.clone()),
        append_prompt: Some(as_text.clone()),
        tools: Some(read_tool()),
        ..explicit()
    };
    let (_, text) = render_text(&env, &project, &options);
    assert!(text.contains(&as_text));
    assert!(!text.contains("File content that must not replace the prompt."));
    // The path-or-text resolution is a separate, explicit step.
    assert_eq!(resolve_prompt_input(Some(&as_text)).as_deref(), Some("File content that must not replace the prompt."));
    assert_eq!(resolve_prompt_input(Some("two\nlines")).as_deref(), Some("two\nlines"));
    assert_eq!(resolve_prompt_input(Some("/no/such/file")).as_deref(), Some("/no/such/file"));
}

/// B-ac48bf7bb2
#[test]
fn custom_prompt_suppresses_discovered_system_md_but_keeps_the_footer() {
    let env = env();
    let project = env.root.join("project");
    fs::create_dir_all(project.join(".ara")).unwrap();
    fs::write(project.join(".ara/SYSTEM.md"), "Discovered project SYSTEM prompt").unwrap();
    let append = "Extra append instructions";
    let options = SystemPromptOptions {
        custom_prompt: Some("CLI custom prompt".into()),
        append_prompt: Some(append.into()),
        tools: Some(read_tool()),
        include_workspace_tree: true,
        workspace_tree: Some(ara_context::WorkspaceTree {
            rendered: ".\n  - nested/".into(),
            truncated: false,
            agents_md_files: vec!["nested/AGENTS.md".into()],
        }),
        ..explicit()
    };
    let (blocks, text) = render_text(&env, &project, &options);
    assert_eq!(blocks.len(), 2);
    assert!(text.contains("CLI custom prompt"));
    assert!(text.contains("<workspace-tree>") && text.contains("<dir-context>") && text.contains("<workstation>"));
    assert_eq!(text.matches(append).count(), 1);
    assert!(!text.contains("Discovered project SYSTEM prompt"));
}

/// B-32832d8c70
#[test]
fn active_child_repo_context_is_rendered() {
    let env = env();
    let parent = env.root.join("parent-cwd");
    fs::create_dir_all(parent.join("active-project/.git")).unwrap();
    let options = SystemPromptOptions { active_repo_context: None, ..explicit() };
    let (_, text) = render_text(&env, &parent, &options);
    assert!(text.contains("<active-repo-context>"));
    assert!(text.contains("`active-project`") && text.contains("`active-project/`"));
    // Two child repos: ambiguous, no block.
    fs::create_dir_all(parent.join("other/.git")).unwrap();
    let (_, text) = render_text(&env, &parent, &options);
    assert!(!text.contains("<active-repo-context>"));
}

/// B-49d98d5221
#[test]
fn project_system_md_wins_over_user() {
    let env = env();
    let project = env.root.join("project");
    fs::create_dir_all(project.join(".ara")).unwrap();
    fs::create_dir_all(env.home.join(".ara/agent")).unwrap();
    fs::write(env.home.join(".ara/agent/SYSTEM.md"), "User SYSTEM prompt").unwrap();
    fs::write(project.join(".ara/SYSTEM.md"), "Project SYSTEM prompt").unwrap();
    assert_eq!(discovery(&env).load_system_prompt_file(&project).unwrap().content, "Project SYSTEM prompt");
}

/// B-128f17b393, B-0dc96c1ab7
#[test]
fn explicit_context_entries_dedupe_by_content() {
    let env = env();
    let far = env.root.join("far/AGENTS.md");
    let near = env.root.join("near/CLAUDE.md");
    let shared = "Shared context instructions";
    let options = SystemPromptOptions {
        custom_prompt: Some("Base prompt".into()),
        context_files: Some(vec![ctx_file(&far, shared, 2), ctx_file(&near, shared, 0)]),
        ..explicit()
    };
    let (_, text) = render_text(&env, &env.root, &options);
    assert_eq!(text.matches(shared).count(), 1);
    assert!(!text.contains(&format!("<file path=\"{}\">", far.display())));
    assert!(text.contains(&format!("<file path=\"{}\">", near.display())));

    let options = SystemPromptOptions {
        custom_prompt: Some("Base prompt".into()),
        context_files: Some(vec![
            ctx_file(&far, "Root context instructions", 2),
            ctx_file(&near, "Near context instructions", 0),
        ]),
        ..explicit()
    };
    let (_, text) = render_text(&env, &env.root, &options);
    assert!(text.contains("Root context instructions") && text.contains("Near context instructions"));
}

/// B-2158dc2dfd
#[test]
fn identical_discovered_context_keeps_the_closest_copy() {
    let env = env();
    let project = env.root.join("project");
    let app = project.join("packages/app");
    fs::create_dir_all(&app).unwrap();
    fs::write(project.join("AGENTS.md"), "Shared context instructions").unwrap();
    fs::write(app.join("AGENTS.md"), "Shared context instructions").unwrap();
    let files = discovery(&env).load_project_context_files(&app, &[]);
    let discovered: Vec<_> = files.iter().filter(|f| f.path.starts_with(&project)).collect();
    assert_eq!(discovered.len(), 1);
    assert_eq!(discovered[0].path, app.join("AGENTS.md"));
}

// --- personality, model, kernel -------------------------------------------------------

const DEFAULT_PRESET_MARKER: &str = "Evidence-first terse engineer";
const OVERRIDE: &str = "Follow ASD-STE100 Simplified Technical English for all responses.";

/// B-6cb4674edc, B-fb85aa978a, B-fe3f4031bc, B-379e6b5b75
#[test]
fn personality_override_file() {
    let env = env();
    let agent_dir = env.home.join(".ara/agent");
    fs::create_dir_all(&agent_dir).unwrap();
    let render = |personality| {
        let options = SystemPromptOptions { personality, ..explicit() };
        render_text(&env, &env.root, &options).1
    };
    let preset = render(Personality::Default);
    assert!(preset.contains(DEFAULT_PRESET_MARKER));
    fs::write(agent_dir.join("PERSONALITY.md"), "   \n").unwrap();
    assert!(render(Personality::Default).contains(DEFAULT_PRESET_MARKER));
    fs::write(agent_dir.join("PERSONALITY.md"), OVERRIDE).unwrap();
    let overridden = render(Personality::Default);
    assert!(overridden.contains("# Personality") && overridden.contains(OVERRIDE));
    assert!(!overridden.contains(DEFAULT_PRESET_MARKER));
    let none = render(Personality::None);
    assert!(!none.contains("# Personality") && !none.contains(OVERRIDE));
}

/// B-8d56a856ca, B-39306baf41
#[test]
fn model_line_in_workstation_block() {
    let env = env();
    let with = SystemPromptOptions { model: Some("anthropic/claude-opus-4".into()), ..explicit() };
    assert!(render_text(&env, &env.root, &with).1.contains("Model: anthropic/claude-opus-4"));
    assert!(!render_text(&env, &env.root, &explicit()).1.contains("Model:"));
}

/// B-5b357ad82c, B-713fad8d66, B-dbff24967f
#[test]
fn kernel_identity_fallbacks() {
    assert_eq!(kernel_identity("unknown", "Darwin", "24.6.0"), "Darwin 24.6.0");
    assert_eq!(kernel_identity("   ", "Darwin", "25.0.0"), "Darwin 25.0.0");
    assert_eq!(kernel_identity("#1 SMP PREEMPT_DYNAMIC", "Linux", "6.1"), "#1 SMP PREEMPT_DYNAMIC");
}

// --- system-prompt-inventory.test.ts (compact native mode) -----------------------------

/// B-3fed644ff2, B-c712aeca56, B-840423dcc3
#[test]
fn compact_tool_inventory() {
    let env = env();
    let tools = vec![
        PromptTool { name: "read".into(), label: "Read".into() },
        PromptTool { name: "bash".into(), label: "Bash".into() },
    ];
    let (_, listed) = render_text(&env, &env.root, &SystemPromptOptions { tools: Some(tools), ..explicit() });
    assert!(listed.contains("- Read: `read`") && listed.contains("- Bash: `bash`"));
    assert!(!listed.contains("namespace functions"));

    let (_, fallback) = render_text(&env, &env.root, &SystemPromptOptions { tools: None, ..explicit() });
    for name in ara_context::DEFAULT_TOOL_NAMES {
        assert!(fallback.contains(&format!("- `{name}`")), "{name}");
    }
    assert!(!fallback.contains("- `task`") && !fallback.contains("- `browser`"));

    let (_, empty) = render_text(&env, &env.root, &explicit());
    assert!(!empty.contains("# Tool Inventory") && !empty.contains("- `read`"));
}

/// B-d839dbb8d1, B-8c051d393f, B-45bea517e6, B-d0113f9195
#[test]
fn skills_listing_rules() {
    let env = env();
    let skills = |list| SystemPromptOptions { skills: Some(list), ..explicit() };
    let (_, no_tools_map) = render_text(
        &env,
        &env.root,
        &SystemPromptOptions {
            tools: None,
            ..skills(vec![skill("prompt-authoring", "Prompt authoring workflow", false)])
        },
    );
    assert!(no_tools_map.contains("- prompt-authoring: Prompt authoring workflow"));

    let bash_only = vec![PromptTool { name: "bash".into(), label: "Bash".into() }];
    let (_, no_read) = render_text(
        &env,
        &env.root,
        &SystemPromptOptions {
            tools: Some(bash_only),
            ..skills(vec![skill("search-only-skill", "Should not render without read", false)])
        },
    );
    assert!(!no_read.contains("search-only-skill"));

    let (_, hidden) = render_text(
        &env,
        &env.root,
        &SystemPromptOptions {
            tools: Some(read_tool()),
            ..skills(vec![skill("hidden-workflow", "Hidden prompt workflow", true)])
        },
    );
    assert!(!hidden.contains("hidden-workflow"));

    let (_, listed) = render_text(
        &env,
        &env.root,
        &SystemPromptOptions {
            tools: Some(read_tool()),
            ..skills(vec![skill("frontend-design", "Frontend UI workflow", false)])
        },
    );
    assert!(listed.contains("<skills>") && listed.contains("- frontend-design: Frontend UI workflow"));
}

/// B-e2178d6763, B-14664feb56, B-e68b0779ba
#[test]
fn absent_tools_drop_their_guidance() {
    let env = env();
    let (_, text) = render_text(&env, &env.root, &SystemPromptOptions { tools: Some(read_tool()), ..explicit() });
    assert!(!text.contains("scout"));
    assert!(!text.contains("browser-drive"));
    assert!(text.contains("No suitable runtime tool for the changed surface"));
    assert!(!text.contains("Update todos"));
}

/// ARA edit: only resolvable internal URLs are advertised.
#[test]
fn internal_urls_follow_the_host() {
    let env = env();
    let (_, none) = render_text(&env, &env.root, &explicit());
    assert!(!none.contains("# Internal URLs") && !none.contains("omp://") && !none.contains("agent://"));
    let urls = ara_context::InternalUrls { skill: true, ..Default::default() };
    let (_, skill_only) = render_text(&env, &env.root, &SystemPromptOptions { urls, ..explicit() });
    assert!(skill_only.contains("# Internal URLs") && skill_only.contains("`skill://<name>`"));
    assert!(!skill_only.contains("history://") && !skill_only.contains("pr://"));
    assert!(skill_only.contains("in ARA coding harness"));
}

// --- date-cwd-reminder.test.ts ---------------------------------------------------------

fn user(content: &str, timestamp: i64) -> Message {
    Message::User(UserMessage { content: UserContent::Text(content.into()), synthetic: None, timestamp })
}

fn assistant(text: &str) -> Message {
    let mut message = AssistantMessage::empty("test", "test", "test");
    message.content.push(ara_ai::AssistantBlock::text(text));
    Message::Assistant(message)
}

fn user_text(message: &Message) -> String {
    match message {
        Message::User(u) => u.content.plain_text(),
        _ => panic!("not a user message"),
    }
}

/// B-2f0be4f5a2, B-94819dcccd, B-53e03fa555, B-bba957baaa, B-010377fb9b, B-9396f95c9a
#[test]
fn date_cwd_reminder_injection() {
    let reminder = render_date_cwd_reminder("2026-08-14", "C:/work/omp");
    assert!(reminder.starts_with("<system-reminder>") && reminder.ends_with("</system-reminder>"));
    assert!(reminder.contains("2026-08-14") && reminder.contains("C:/work/omp") && reminder.contains("Do not repeat"));

    let injector = DateCwdReminder::new();
    let context = Context {
        system_prompt: vec!["system".into()],
        messages: vec![user("hello", 1), assistant("hi")],
        tools: None,
    };
    let out = injector.transform(context.clone(), "2026-08-14", "/work/omp");
    assert_eq!(
        user_text(&out.messages[0]),
        format!("{}\n\nhello", render_date_cwd_reminder("2026-08-14", "/work/omp"))
    );
    assert_eq!(out.messages[1], context.messages[1]);
    assert_eq!(user_text(&context.messages[0]), "hello");

    let image = Message::User(UserMessage {
        content: UserContent::Blocks(vec![UserBlock::Image(ara_ai::ImageContent {
            data: "img".into(),
            mime_type: "image/png".into(),
        })]),
        synthetic: None,
        timestamp: 1,
    });
    let out = DateCwdReminder::new().transform(
        Context { system_prompt: vec!["system".into()], messages: vec![image], tools: None },
        "2026-08-14",
        "/work/omp",
    );
    let Message::User(UserMessage { content: UserContent::Blocks(blocks), .. }) = &out.messages[0] else { panic!() };
    assert!(matches!(&blocks[0], UserBlock::Text(t) if t.text == render_date_cwd_reminder("2026-08-14", "/work/omp")));
    assert!(matches!(&blocks[1], UserBlock::Image(_)));

    let no_system = Context { system_prompt: vec![], messages: vec![user("hi", 1)], tools: None };
    assert_eq!(DateCwdReminder::new().transform(no_system.clone(), "d", "/c"), no_system);
    let no_user = Context { system_prompt: vec!["s".into()], messages: vec![assistant("hi")], tools: None };
    assert_eq!(DateCwdReminder::new().transform(no_user.clone(), "d", "/c"), no_user);

    let injector = DateCwdReminder::new();
    let first = injector.transform(
        Context { system_prompt: vec!["s".into()], messages: vec![user("first", 1)], tools: None },
        "2026-08-14",
        "/old",
    );
    let first_injected = first.messages[0].clone();
    let replay = injector.transform(
        Context { system_prompt: vec!["s".into()], messages: vec![user("first", 1)], tools: None },
        "2026-08-14",
        "/old",
    );
    assert_eq!(replay.messages[0], first_injected);
    let second = injector.transform(
        Context {
            system_prompt: vec!["s".into()],
            messages: vec![user("first", 1), assistant("done"), user("second", 2)],
            tools: None,
        },
        "2026-08-15",
        "/new",
    );
    assert_eq!(second.messages[0], first_injected);
    assert_eq!(user_text(&second.messages[2]), format!("{}\n\nsecond", render_date_cwd_reminder("2026-08-15", "/new")));
}
