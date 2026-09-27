//! `ara` — reference host for the ARA Core (print mode).
//!
//! Ported behavior: OMP `packages/coding-agent/src/modes/print-mode.ts`
//! (`runPrintMode`, `printableEvent`), `cli/initial-message.ts` (stdin is
//! prepended to the first prompt) and `pi-utils` `sanitizeText`, at
//! 596f2da7101178214aa27a753529d15e6b7ad91d.
//!
//! The host binds the Core ports: model route (OpenAI-compatible Chat
//! Completions), tools (`read`/`write`/`edit`/`bash`/`grep`/`glob` rooted at the session cwd), the
//! event sink (JSON output + session journal), cancellation (SIGINT) and
//! budgets (`--max-time`, `--max-model-calls`). Credentials come only from the
//! environment and are never printed; a key is only sent to its own route.
//!
//! Intentional differences from OMP print mode:
//! - JSON mode also exits 1 when the run ends in error/abort (OMP exits 1 only
//!   in text mode), so automation cannot mistake a failed run for success.
//! - Later prompts are not sent after a prompt ends in error, abort, deadline
//!   or budget stop (OMP keeps sending queued prompts).
//! - A journal write failure cancels the run before any further tool runs;
//!   an unrecorded tool effect would be unknowable on resume.
//! - Deadline and model-call budget stops are reported on stderr with exit 1.

use anyhow::{Context as _, Result, bail};
use ara_agent::{AgentConfig, AgentEvent, AgentEventSink, LoopHooks, RunEnd, agent_loop};
use ara_ai::providers::openai_completions::{PreparedRequestTextObservation, RequestTextObserver, StreamOptions};
use ara_ai::providers::openai_responses::StreamOptions as ResponsesStreamOptions;
use ara_ai::{
    AnthropicMessagesProvider, Message, Model, ModelProvider, ModelTokenizer, OpenAICompletionsProvider,
    OpenAIResponsesProvider, StopReason, UserMessage, resolve_known_claude_tokenizer,
};
use ara_context::{
    DateCwdReminder, InternalUrls, PromptTool, SystemPromptOptions, build_system_prompt, resolve_prompt_input,
};
use ara_discovery::{Discovery, HostDirs, ProviderPolicy, SkillsSettings};
use ara_session::{SessionJournal, latest_session};
use ara_tools::{ToolContext, builtin_tools};
use async_trait::async_trait;
use clap::{Parser, ValueEnum};
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

const TOOL_NAMES: [&str; 6] = ["read", "write", "edit", "bash", "grep", "glob"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Mode {
    /// Final assistant text only.
    Text,
    /// Every agent event as one JSON line.
    Json,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Api {
    #[value(name = "anthropic-messages")]
    AnthropicMessages,
    #[value(name = "openai-completions")]
    OpenaiCompletions,
    #[value(name = "openai-responses")]
    OpenaiResponses,
}

impl Api {
    fn as_str(self) -> &'static str {
        match self {
            Self::AnthropicMessages => "anthropic-messages",
            Self::OpenaiCompletions => "openai-completions",
            Self::OpenaiResponses => "openai-responses",
        }
    }
}

#[derive(Parser, Debug)]
#[command(name = "ara", version, about = "ARA agent (print mode)")]
struct Args {
    /// Prompts, sent in order. Piped stdin is prepended to the first prompt
    /// (or used alone when no prompt is given).
    prompts: Vec<String>,
    /// Print mode (the only mode this host implements; accepted for OMP parity).
    #[arg(short = 'p', long)]
    print: bool,
    #[arg(long, value_enum, default_value_t = Mode::Text)]
    mode: Mode,
    /// Model id sent on the wire. Env: ARA_MODEL, ARA_TEST_MODEL_ID.
    #[arg(long)]
    model: Option<String>,
    /// Model wire protocol. Defaults to Chat Completions.
    #[arg(long, value_enum, default_value_t = Api::OpenaiCompletions)]
    api: Api,
    /// Local Claude content tokenizer metadata (no context gate yet): auto,
    /// none, claude-v3, claude-v47, claude-v5, or claude-v5-sonnet. Env: ARA_TOKENIZER.
    #[arg(long)]
    tokenizer: Option<String>,
    /// Report selected-family counts for prepared message text on stderr.
    /// Tool payloads, images and request framing are not included.
    #[arg(long)]
    report_request_text_tokens: bool,
    /// Provider base URL (…/v1 for OpenAI or Anthropic). Env: ARA_BASE_URL, ARA_TEST_BASE_URL, OPENROUTER_BASE_URL.
    #[arg(long)]
    base_url: Option<String>,
    /// Name of the environment variable holding the API key. Provider keys
    /// are selected automatically only for their official HTTPS route.
    #[arg(long)]
    api_key_env: Option<String>,
    /// Provider label recorded on messages (default: openrouter for openrouter.ai, else openai-compatible).
    #[arg(long)]
    provider: Option<String>,
    /// Working directory for tools (default: the resumed session's cwd, else the current directory).
    #[arg(long)]
    cwd: Option<PathBuf>,
    /// Session directory (default: $ARA_HOME/sessions/<encoded cwd>, ARA_HOME=~/.ara).
    #[arg(long)]
    session_dir: Option<PathBuf>,
    /// Do not persist the session.
    #[arg(long, conflicts_with_all = ["resume", "continue_session"])]
    no_session: bool,
    /// Resume a session file.
    #[arg(long, conflicts_with = "continue_session")]
    resume: Option<PathBuf>,
    /// Resume the most recent session in the session directory.
    #[arg(short = 'c', long = "continue")]
    continue_session: bool,
    /// Wall-clock limit for each prompt's run, in seconds.
    #[arg(long)]
    max_time: Option<f64>,
    /// Maximum model calls per prompt (budget gate).
    #[arg(long)]
    max_model_calls: Option<usize>,
    #[arg(long)]
    max_tokens: Option<u64>,
    /// Confirm that the selected Responses model supports reasoning items.
    /// Requests encrypted reasoning for same-endpoint continuation.
    #[arg(long)]
    reasoning: bool,
    /// Store Responses on the provider and chain compatible turns in this process.
    /// Requires --api openai-responses; disabled by default.
    #[arg(long)]
    responses_stateful: bool,
    #[arg(long)]
    temperature: Option<f64>,
    /// Replace the default system prompt (text, or a file path). Without it,
    /// a discovered `SYSTEM.md` is used. The last occurrence wins.
    #[arg(long, overrides_with = "system_prompt")]
    system_prompt: Option<String>,
    /// Append text to the system prompt (text, or a file path). Without it, a
    /// discovered `APPEND_SYSTEM.md` is used. The last occurrence wins.
    #[arg(long, overrides_with = "append_system_prompt")]
    append_system_prompt: Option<String>,
    /// Do not discover or list skills.
    #[arg(long)]
    no_skills: bool,
    /// Only include skills whose names match these globs (comma separated).
    #[arg(long)]
    skills: Option<String>,
    /// Tools to enable (comma separated): read,write,edit,bash,grep,glob. Empty disables tools.
    #[arg(long, default_value = "read,write,edit,bash,grep,glob")]
    tools: String,
    /// Prefix read output with line numbers.
    #[arg(long)]
    line_numbers: bool,
    /// Edit tool mode: hashline (default; anchored reads), replace, patch, apply_patch or sloppy.
    #[arg(long, default_value = "hashline", value_parser = parse_edit_mode)]
    edit_mode: pi_edit::EditMode,
    /// Include thinking blocks in text output.
    #[arg(long)]
    print_thoughts: bool,
    /// Extra request header `Name: value` (repeatable).
    #[arg(long = "header")]
    headers: Vec<String>,
    /// Stream idle and first-event timeout in seconds (default 300; 0 disables).
    #[arg(long)]
    stream_idle_timeout: Option<f64>,
}

fn parse_edit_mode(value: &str) -> Result<pi_edit::EditMode, String> {
    pi_edit::EditMode::parse(value)
        .ok_or_else(|| format!("unknown edit mode {value:?} (hashline, replace, patch, apply_patch, sloppy)"))
}

/// Per-request provider context rewrite: the date/cwd reminder on the first
/// user turn (OMP `DateCwdReminderInjector`).
struct CliHooks {
    reminder: DateCwdReminder,
    cwd: String,
}

#[async_trait]
impl LoopHooks for CliHooks {
    async fn transform_provider_context(&self, context: ara_ai::Context, _model: &Model) -> ara_ai::Context {
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        self.reminder.transform(context, &date, &self.cwd)
    }
}

/// `$ARA_HOME` (default `~/.ara`), absolute against the process cwd so paths
/// derived from it do not depend on the session's `--cwd`.
fn ara_home() -> PathBuf {
    let home = std::env::var_os("ARA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".ara")))
        .unwrap_or_else(|| PathBuf::from(".ara"));
    std::path::absolute(&home).unwrap_or(home)
}

fn user_home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// OMP `sanitizeText`: strip ANSI escape sequences, then C0/C1 controls other
/// than `\t` and `\n`.
fn sanitize_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    for d in chars.by_ref() {
                        if ('\x40'..='\x7e').contains(&d) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    while let Some(d) = chars.next() {
                        if d == '\x07' || (d == '\x1b' && chars.peek() == Some(&'\\')) {
                            if d == '\x1b' {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            }
            continue;
        }
        let code = c as u32;
        let control = (code <= 0x08) || (0x0B..=0x1F).contains(&code) || (0x7F..=0x9F).contains(&code);
        if !control {
            out.push(c);
        }
    }
    out
}

fn env_first(names: &[&str]) -> Option<(String, String)> {
    names.iter().find_map(|n| std::env::var(n).ok().filter(|v| !v.is_empty()).map(|v| (n.to_string(), v)))
}

fn is_openrouter(base_url: &str) -> bool {
    base_url
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split(['/', ':']).next())
        .is_some_and(|host| host == "openrouter.ai" || host.ends_with(".openrouter.ai"))
}

fn is_official_anthropic(base_url: &str) -> bool {
    reqwest::Url::parse(base_url).ok().is_some_and(|url| {
        url.scheme() == "https" && url.host_str().is_some_and(|host| host.eq_ignore_ascii_case("api.anthropic.com"))
    })
}

fn default_session_dir(cwd: &Path) -> PathBuf {
    let home = std::env::var_os("ARA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".ara")))
        .unwrap_or_else(|| PathBuf::from(".ara"));
    let encoded = format!("--{}--", cwd.to_string_lossy().trim_matches('/').replace(['/', '\\', ':'], "-"));
    home.join("sessions").join(encoded)
}

/// Writes JSON events and journals every completed message. A journal write
/// failure cancels the run so no further effect goes unrecorded.
struct HostSink {
    mode: Mode,
    journal: tokio::sync::Mutex<Option<SessionJournal>>,
    persist_failed: AtomicBool,
    cancel: CancellationToken,
}

impl HostSink {
    fn write_line(&self, line: &str) {
        let mut out = std::io::stdout().lock();
        // A closed stdout (e.g. `| head`) must not abort the run or the journal.
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    }
}

#[async_trait]
impl AgentEventSink for HostSink {
    async fn emit(&self, event: AgentEvent) {
        if let AgentEvent::MessageEnd { message } = &event
            && let Some(journal) = self.journal.lock().await.as_mut()
            && let Err(e) = journal.append_message(message)
        {
            self.persist_failed.store(true, Ordering::SeqCst);
            eprintln!("ara: session persistence failed ({e}); aborting the run");
            self.cancel.cancel();
        }
        if self.mode == Mode::Json {
            self.write_line(&event.printable().to_string());
        }
    }
}

fn read_stdin() -> Result<Option<String>> {
    let mut stdin = std::io::stdin();
    if stdin.is_terminal() {
        return Ok(None);
    }
    let mut text = String::new();
    stdin.read_to_string(&mut text).context("reading stdin")?;
    let text = text.trim().to_string();
    Ok((!text.is_empty()).then_some(text))
}

struct Route {
    model: Model,
    stream_options: StreamOptions,
}

/// Validate every argument that does not need the journal (no I/O side effects).
fn resolve_route(args: &Args) -> Result<Route> {
    if args.reasoning && args.api != Api::OpenaiResponses {
        bail!("--reasoning requires --api openai-responses");
    }
    if args.responses_stateful && args.api != Api::OpenaiResponses {
        bail!("--responses-stateful requires --api openai-responses");
    }
    if args.api != Api::OpenaiCompletions && args.report_request_text_tokens {
        bail!("--report-request-text-tokens requires --api openai-completions");
    }
    let model_id = args
        .model
        .clone()
        .or_else(|| env_first(&["ARA_MODEL", "ARA_TEST_MODEL_ID"]).map(|(_, v)| v))
        .context("no model: pass --model or set ARA_MODEL")?;
    let tokenizer_choice = args.tokenizer.clone().or_else(|| env_first(&["ARA_TOKENIZER"]).map(|(_, value)| value));
    let tokenizer = match tokenizer_choice.as_deref() {
        None | Some("auto") => resolve_known_claude_tokenizer(&model_id),
        Some("none") => None,
        Some(name) => Some(ModelTokenizer::from_name(name).with_context(|| {
            format!("unknown tokenizer {name:?}; use auto, none, claude-v3, claude-v47, claude-v5 or claude-v5-sonnet")
        })?),
    };
    let base_url = args
        .base_url
        .clone()
        .or_else(|| env_first(&["ARA_BASE_URL", "ARA_TEST_BASE_URL", "OPENROUTER_BASE_URL"]).map(|(_, v)| v))
        .context("no base URL: pass --base-url or set ARA_BASE_URL")?;
    let openrouter = is_openrouter(&base_url);
    // Route-specific keys stay on their own hosts. ARA keys or an explicit
    // --api-key-env may be used for another endpoint.
    let api_key = match &args.api_key_env {
        Some(name) => Some(std::env::var(name).with_context(|| format!("environment variable {name} is not set"))?),
        None if args.api == Api::AnthropicMessages && is_official_anthropic(&base_url) => {
            env_first(&["ANTHROPIC_API_KEY", "ARA_API_KEY", "ARA_TEST_API_KEY"]).map(|(_, v)| v)
        }
        None if args.api == Api::AnthropicMessages => env_first(&["ARA_API_KEY", "ARA_TEST_API_KEY"]).map(|(_, v)| v),
        None if openrouter => env_first(&["OPENROUTER_API_KEY"]).map(|(_, v)| v),
        None => env_first(&["ARA_API_KEY", "ARA_TEST_API_KEY"]).map(|(_, v)| v),
    };
    let provider = args.provider.clone().unwrap_or_else(|| {
        if args.api == Api::AnthropicMessages {
            "anthropic".into()
        } else if openrouter {
            "openrouter".into()
        } else {
            "openai-compatible".into()
        }
    });
    let mut extra_headers = Vec::new();
    for h in &args.headers {
        let (k, v) = h.split_once(':').with_context(|| format!("--header {h:?} must be `Name: value`"))?;
        extra_headers.push((k.trim().to_string(), v.trim().to_string()));
    }
    for name in args.tools.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if !TOOL_NAMES.contains(&name) {
            bail!("unknown tool {name:?} (available: {})", TOOL_NAMES.join(", "));
        }
    }
    let mut stream_options = StreamOptions { api_key, extra_headers, ..Default::default() };
    if let Some(s) = args.stream_idle_timeout {
        let d = (s > 0.0).then(|| Duration::from_secs_f64(s));
        stream_options.idle_timeout = d;
        stream_options.first_event_timeout = d;
    }
    Ok(Route {
        model: Model {
            id: model_id,
            api: args.api.as_str().into(),
            provider,
            base_url,
            reasoning: args.reasoning,
            max_tokens: None,
            tokenizer,
        },
        stream_options,
    })
}

async fn run(args: Args) -> Result<i32> {
    let _ = args.print;
    let route = resolve_route(&args)?;
    let mut prompts = args.prompts.clone();
    if let Some(stdin) = read_stdin()? {
        // OMP buildInitialMessage: `${stdin}\n${firstPrompt}`.
        match prompts.first_mut() {
            Some(first) => *first = format!("{stdin}\n{first}"),
            None => prompts.push(stdin),
        }
    }
    if prompts.is_empty() {
        bail!("no prompt given (pass it as an argument or on stdin)");
    }
    let explicit_cwd = match &args.cwd {
        Some(c) => Some(std::fs::canonicalize(c).with_context(|| format!("--cwd {}", c.display()))?),
        None => None,
    };
    let launch_cwd = explicit_cwd.clone().map(Ok).unwrap_or_else(std::env::current_dir)?;

    // Session journal (first journal I/O happens only after validation above).
    let mut cwd = launch_cwd.clone();
    let mut journal = if args.no_session {
        None
    } else {
        let dir = args.session_dir.clone().unwrap_or_else(|| default_session_dir(&launch_cwd));
        let path = match (&args.resume, args.continue_session) {
            (Some(p), _) => Some(p.clone()),
            (None, true) => {
                Some(latest_session(&dir).with_context(|| format!("no session to continue in {}", dir.display()))?)
            }
            _ => None,
        };
        Some(match path {
            Some(p) => {
                let mut j = SessionJournal::open(&p).with_context(|| format!("opening session {}", p.display()))?;
                if let Some(session_cwd) = j.header().get("cwd").and_then(|v| v.as_str()).map(PathBuf::from) {
                    match &explicit_cwd {
                        Some(c) if c != &session_cwd => eprintln!(
                            "ara: warning: session was recorded in {} but tools run in --cwd {}",
                            session_cwd.display(),
                            c.display()
                        ),
                        Some(_) => {}
                        None if session_cwd.is_dir() => cwd = session_cwd,
                        None => bail!(
                            "session cwd {} no longer exists; pass --cwd to choose where tools run",
                            session_cwd.display()
                        ),
                    }
                }
                if j.report.malformed_records > 0 {
                    eprintln!("ara: skipped {} malformed record(s) in {}", j.report.malformed_records, p.display());
                }
                let undecodable = j.undecodable_messages();
                if undecodable > 0 {
                    eprintln!(
                        "ara: {undecodable} message(s) in the session could not be decoded and are not sent to the model"
                    );
                }
                let recovery = j.recover_interrupted_tool_calls()?;
                if !recovery.paired.is_empty() {
                    eprintln!(
                        "ara: tool call(s) {} were interrupted before a result was recorded; their effects are unknown and they were not re-run",
                        recovery.paired.join(", ")
                    );
                }
                if !recovery.unpaired_earlier.is_empty() {
                    eprintln!(
                        "ara: earlier tool call(s) {} have no recorded result",
                        recovery.unpaired_earlier.join(", ")
                    );
                }
                j
            }
            None => SessionJournal::create(&dir, &cwd)?,
        })
    };
    let model_ref = format!("{}/{}", route.model.provider, route.model.id);
    let mut context: Vec<Message> = journal.as_ref().map(SessionJournal::build_context).unwrap_or_default();
    if let Some(j) = journal.as_mut()
        && j.current_model().as_deref() != Some(model_ref.as_str())
    {
        j.append_model_change(&model_ref)?;
    }
    let header = journal.as_ref().map(|j| j.header().clone()).unwrap_or_else(|| {
        serde_json::json!({"type": "session", "version": ara_session::CURRENT_SESSION_VERSION, "id": "ephemeral", "cwd": cwd.to_string_lossy()})
    });
    let session_path = journal.as_ref().map(|j| j.path().to_path_buf());

    // Context files, skills, SYSTEM.md and APPEND_SYSTEM.md from the host's
    // locations: native `$ARA_HOME/agent` and `.ara/`, foreign tools per
    // upstream defaults.
    let home = user_home();
    let mut dirs = HostDirs::ara(&home).with_env(|k| std::env::var(k).ok());
    dirs.native_user_dir = ara_home().join("agent");
    let discovery = Discovery::new(&home, dirs, ProviderPolicy::default());
    let skills_settings = SkillsSettings {
        enabled: !args.no_skills,
        include_skills: args
            .skills
            .as_deref()
            .map(|s| s.split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect())
            .unwrap_or_default(),
        ..SkillsSettings::default()
    };
    let (skills, skill_warnings) = discovery.load_skills(&cwd, &skills_settings);
    for warning in &skill_warnings {
        let at = if warning.skill_path.is_empty() { String::new() } else { format!(" ({})", warning.skill_path) };
        eprintln!("ara: skill warning{at}: {}", warning.message);
    }

    let enabled: Vec<&str> = args.tools.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
    let mut tool_ctx = ToolContext::new(cwd.clone()).with_edit(args.edit_mode, enabled.contains(&"edit")).with_skills(
        skills
            .iter()
            .map(|s| ara_tools::internal_urls::SkillRef {
                name: s.name.clone(),
                file_path: s.file_path.clone(),
                base_dir: s.base_dir.clone(),
            })
            .collect(),
    );
    tool_ctx.line_numbers = args.line_numbers;
    let tools: Vec<_> =
        builtin_tools(tool_ctx).into_iter().filter(|t| enabled.contains(&t.definition().name.as_str())).collect();

    let prompt_tools: Vec<PromptTool> = tools
        .iter()
        .map(|t| {
            let name = t.definition().name.clone();
            let label = name.get(..1).map(|f| f.to_uppercase() + &name[1..]).unwrap_or_default();
            PromptTool { name, label }
        })
        .collect();
    // Flags win; otherwise the discovered files (upstream `main.ts`
    // `discoverSystemPromptFile` / `discoverAppendSystemPromptFile`).
    let mut prompt_warnings = Vec::new();
    let discovered = |name: &str| discovery.discover_prompt_file(&cwd, name).map(|p| p.to_string_lossy().into_owned());
    let system_source = args.system_prompt.clone().or_else(|| discovered("SYSTEM.md"));
    let append_source = args.append_system_prompt.clone().or_else(|| discovered("APPEND_SYSTEM.md"));
    let custom_prompt = resolve_prompt_input(system_source.as_deref(), "system prompt", &mut prompt_warnings);
    let append_prompt = resolve_prompt_input(append_source.as_deref(), "append system prompt", &mut prompt_warnings);
    let options = SystemPromptOptions {
        custom_prompt,
        append_prompt,
        tools: Some(prompt_tools),
        skills: Some(skills),
        model: Some(route.model.id.clone()),
        urls: InternalUrls { skill: enabled.contains(&"read"), ..InternalUrls::default() },
        ..SystemPromptOptions::default()
    };
    let built = build_system_prompt(&discovery, &cwd, &options).context("building the system prompt")?;
    for warning in prompt_warnings.iter().chain(&built.warnings) {
        eprintln!("ara: warning: {warning}");
    }
    let system_prompt = built.blocks;
    let hooks: Arc<dyn LoopHooks> =
        Arc::new(CliHooks { reminder: DateCwdReminder::new(), cwd: cwd.to_string_lossy().replace('\\', "/") });

    let mut stream_options = route.stream_options;
    let mut request_text_stats = None;
    let request_text_task = if args.report_request_text_tokens {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<PreparedRequestTextObservation>(256);
        let observer = RequestTextObserver::new(tx);
        stream_options.request_text_observer = Some(observer.clone());
        request_text_stats = Some(observer.stats());
        eprintln!("ara: prepared message text only; tool payloads, images and request framing are unmeasured");
        Some(tokio::spawn(async move {
            while let Some(observation) = rx.recv().await {
                let m = observation.measurement;
                eprintln!(
                    "ara: prepared text #{}: {:?}; complete={}, fields={}, bytes={}, tools={}, calls={}, images={}",
                    observation.sequence,
                    m.count,
                    m.coverage_complete,
                    m.text_fields,
                    m.text_bytes,
                    m.has_tool_definitions,
                    m.has_tool_calls,
                    m.has_images
                );
            }
        }))
    } else {
        None
    };
    let client = reqwest::Client::builder().build().context("building HTTP client")?;
    let provider: Arc<dyn ModelProvider> = match args.api {
        Api::AnthropicMessages => Arc::new(AnthropicMessagesProvider {
            client,
            base: ara_ai::providers::anthropic::StreamOptions {
                api_key: stream_options.api_key,
                extra_headers: stream_options.extra_headers,
                first_event_timeout: stream_options.first_event_timeout,
                idle_timeout: stream_options.idle_timeout,
                retry: stream_options.retry,
                ..Default::default()
            },
        }),
        Api::OpenaiCompletions => Arc::new(OpenAICompletionsProvider { client, base: stream_options }),
        Api::OpenaiResponses => Arc::new(OpenAIResponsesProvider {
            client,
            base: ResponsesStreamOptions {
                api_key: stream_options.api_key,
                extra_headers: stream_options.extra_headers,
                first_event_timeout: stream_options.first_event_timeout,
                idle_timeout: stream_options.idle_timeout,
                retry: stream_options.retry,
                session_state: Some(Arc::new(ara_ai::providers::openai_responses::ProviderSessionState::default())),
                stateful_responses: args.responses_stateful,
                ..ResponsesStreamOptions::default()
            },
        }),
    };
    let cancel = CancellationToken::new();
    let sink = HostSink {
        mode: args.mode,
        journal: tokio::sync::Mutex::new(journal),
        persist_failed: AtomicBool::new(false),
        cancel: cancel.clone(),
    };
    if args.mode == Mode::Json {
        sink.write_line(&header.to_string());
    }

    let c2 = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            eprintln!("ara: interrupt received, aborting (press Ctrl-C again to exit immediately)");
            c2.cancel();
            if tokio::signal::ctrl_c().await.is_ok() {
                std::process::exit(130);
            }
        }
    });

    if args.mode == Mode::Text {
        eprintln!("Working...");
    }
    let mut end = RunEnd::Completed;
    for prompt in prompts {
        let config = AgentConfig {
            model: route.model.clone(),
            provider: provider.clone(),
            system_prompt: system_prompt.clone(),
            tools: tools.clone(),
            tool_choice: None,
            max_tokens: args.max_tokens,
            temperature: args.temperature,
            deadline: args.max_time.map(|s| Instant::now() + Duration::from_secs_f64(s.max(0.0))),
            max_model_calls: args.max_model_calls,
            hooks: hooks.clone(),
        };
        let report =
            agent_loop(vec![Message::User(UserMessage::text(prompt))], &mut context, &config, &cancel, &sink).await;
        end = report.end;
        if end != RunEnd::Completed {
            break;
        }
    }
    drop(provider);
    if let Some(task) = request_text_task {
        let _ = task.await;
    }
    let dropped_observations = request_text_stats.as_ref().map_or(0, |stats| stats.dropped());
    if dropped_observations > 0 {
        eprintln!("ara: {dropped_observations} prepared text observation(s) were dropped");
    }

    if let Some(path) = &session_path
        && path.exists()
    {
        eprintln!("ara: session {}", path.display());
    }
    let mut code = 0;
    let last = context.iter().rev().find_map(Message::as_assistant).cloned();
    match end {
        RunEnd::Completed => {
            if let Some(a) = &last {
                if let Some(e) = &a.error_message {
                    eprintln!("{}", sanitize_text(e));
                }
                if args.mode == Mode::Text {
                    for block in &a.content {
                        match block {
                            ara_ai::AssistantBlock::Text(t) => sink.write_line(&sanitize_text(&t.text)),
                            ara_ai::AssistantBlock::Thinking(t)
                                if args.print_thoughts && !t.thinking.trim().is_empty() =>
                            {
                                sink.write_line(&sanitize_text(&t.thinking))
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        RunEnd::Deadline => {
            eprintln!("Deadline exceeded");
            code = 1;
        }
        RunEnd::ModelCallBudget => {
            eprintln!("ara: model call limit reached ({})", args.max_model_calls.unwrap_or_default());
            code = 1;
        }
        RunEnd::Aborted | RunEnd::Error => {
            let line = last
                .as_ref()
                .filter(|a| matches!(a.stop_reason, StopReason::Error | StopReason::Aborted))
                .and_then(|a| a.error_message.clone())
                .unwrap_or_else(|| "Request aborted".into());
            eprintln!("{}", sanitize_text(&line));
            code = 1;
        }
    }
    if sink.persist_failed.load(Ordering::SeqCst) {
        code = 1;
    }
    Ok(code)
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let code = match run(args).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("ara: {e:#}");
            2
        }
    };
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_ansi_and_controls() {
        assert_eq!(sanitize_text("a\x1b[31mred\x1b[0m\tb\nc\x07\r"), "ared\tb\nc");
        assert_eq!(sanitize_text("x\x1b]52;c;ZXZpbA==\x07y"), "xy");
        assert_eq!(sanitize_text("x\x1b]0;title\x1b\\y\u{9b}z"), "xyz");
        assert_eq!(sanitize_text("中文 ok"), "中文 ok");
    }

    #[tokio::test]
    async fn journal_failure_cancels_the_run() {
        let dir = std::env::temp_dir().join(format!("ara-sink-{}", std::process::id()));
        let mut j = SessionJournal::create(&dir, &dir).unwrap();
        let mut a = ara_ai::AssistantMessage::empty("x", "y", "z");
        a.content.push(ara_ai::AssistantBlock::text("seed"));
        j.append_message(&Message::Assistant(a.clone())).unwrap();
        let path = j.path().to_path_buf();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        let sink = HostSink {
            mode: Mode::Text,
            journal: tokio::sync::Mutex::new(Some(j)),
            persist_failed: AtomicBool::new(false),
            cancel: CancellationToken::new(),
        };
        sink.emit(AgentEvent::MessageEnd { message: Message::Assistant(a) }).await;
        assert!(sink.persist_failed.load(Ordering::SeqCst));
        assert!(sink.cancel.is_cancelled(), "tools of an unrecorded message must not run");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn openrouter_detection_uses_the_host() {
        assert!(is_openrouter("https://openrouter.ai/api/v1"));
        assert!(!is_openrouter("http://evil.example/openrouter.ai/v1"));
        assert!(!is_openrouter("http://127.0.0.1:8080/v1"));
    }

    #[test]
    fn official_anthropic_detection_requires_its_https_host() {
        assert!(is_official_anthropic("https://api.anthropic.com/v1"));
        assert!(!is_official_anthropic("http://api.anthropic.com/v1"));
        assert!(!is_official_anthropic("https://api.anthropic.com.evil.example/v1"));
        assert!(!is_official_anthropic("https://api.anthropic.com@evil.example/v1"));
        assert!(!is_official_anthropic("http://127.0.0.1:8080/v1"));
    }

    #[test]
    fn route_selects_or_overrides_local_tokenizer_without_changing_wire_id() {
        let args = Args::try_parse_from([
            "ara",
            "--model",
            "anthropic/claude-opus-4-7",
            "--base-url",
            "http://localhost/v1",
            "--tokenizer",
            "auto",
        ])
        .unwrap();
        let route = resolve_route(&args).unwrap();
        assert_eq!(route.model.id, "anthropic/claude-opus-4-7");
        assert_eq!(route.model.tokenizer, Some(ModelTokenizer::ClaudeV47));

        let args = Args::try_parse_from([
            "ara",
            "--model",
            "reseller-alias",
            "--base-url",
            "http://localhost/v1",
            "--tokenizer",
            "claude-v5-sonnet",
        ])
        .unwrap();
        let route = resolve_route(&args).unwrap();
        assert_eq!(route.model.id, "reseller-alias");
        assert_eq!(route.model.tokenizer, Some(ModelTokenizer::ClaudeV5Sonnet));

        let args = Args::try_parse_from([
            "ara",
            "--model",
            "claude-opus-5",
            "--base-url",
            "http://localhost/v1",
            "--tokenizer",
            "none",
        ])
        .unwrap();
        assert_eq!(resolve_route(&args).unwrap().model.tokenizer, None);
    }

    #[test]
    fn invalid_local_tokenizer_is_a_route_error() {
        let args = Args::try_parse_from([
            "ara",
            "--model",
            "alias",
            "--base-url",
            "http://localhost/v1",
            "--tokenizer",
            "claude-v6",
        ])
        .unwrap();
        assert!(resolve_route(&args).err().unwrap().to_string().contains("unknown tokenizer"));
    }

    #[test]
    fn request_text_report_is_opt_in() {
        let args = Args::try_parse_from(["ara", "--report-request-text-tokens", "hi"]).unwrap();
        assert!(args.report_request_text_tokens);
        let args = Args::try_parse_from(["ara", "hi"]).unwrap();
        assert!(!args.report_request_text_tokens);
    }

    #[tokio::test]
    async fn reported_request_closes_its_observer_on_cli_completion() {
        use ara_testkit::chunks::{done, finish, text};
        use ara_testkit::{FakeUpstream, Script};

        let script: Script = serde_json::from_value(serde_json::json!({
            "responses": [{"events": [text("ok"), finish("stop"), done()]}]
        }))
        .unwrap();
        let server = FakeUpstream::start(script, None).await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let args = Args::try_parse_from(vec![
            "ara".to_owned(),
            "--model".to_owned(),
            "claude-opus-4-6".to_owned(),
            "--base-url".to_owned(),
            server.base_url(),
            "--cwd".to_owned(),
            dir.path().to_string_lossy().into_owned(),
            "--no-session".to_owned(),
            "--no-skills".to_owned(),
            "--tools".to_owned(),
            String::new(),
            "--report-request-text-tokens".to_owned(),
            "hi".to_owned(),
        ])
        .unwrap();
        let code = tokio::time::timeout(Duration::from_secs(5), run(args)).await.expect("observer must close").unwrap();
        assert_eq!(code, 0);
        assert_eq!(server.served(), 1);
    }
}
