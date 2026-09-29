//! `ara` — reference host for the ARA Core (print mode and a line REPL).
//!
//! Ported behavior: OMP `packages/coding-agent/src/modes/print-mode.ts`
//! (`runPrintMode`, `printableEvent`), `cli/initial-message.ts` (stdin is
//! prepended to the first prompt) and `pi-utils` `sanitizeText`, at
//! 596f2da7101178214aa27a753529d15e6b7ad91d.
//!
//! The host binds the Core ports: model route (OpenAI-compatible Chat
//! Completions), tools (`read`/`write`/`edit`/`bash`/`grep`/`glob` rooted at the session cwd,
//! plus opt-in `ast_grep`), the
//! event sink (JSON output + session journal), cancellation (SIGINT; Ctrl+C
//! or Ctrl+Break on Windows) and
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
//! - The line REPL is a reference-host subset, not OMP's TUI. A line reader
//!   cannot see Esc, so Ctrl+C during a turn aborts that turn (OMP: Esc) and
//!   Ctrl+C at the idle prompt exits (OMP: a double Ctrl+C); a second Ctrl+C
//!   while a turn is still aborting exits at once, as in print mode.

use anyhow::{Context as _, Result, bail};
use ara_agent::{AgentConfig, AgentEvent, AgentEventSink, LoopHooks, RunEnd, agent_loop};
use ara_ai::providers::openai_completions::{PreparedRequestTextObservation, RequestTextObserver, StreamOptions};
use ara_ai::providers::openai_responses::StreamOptions as ResponsesStreamOptions;
use ara_ai::{
    AnthropicMessagesProvider, AssistantMessageEvent, Message, Model, ModelProvider, ModelTokenizer,
    OpenAICompletionsProvider, OpenAIResponsesProvider, StopReason, UserMessage, resolve_known_claude_tokenizer,
};
use ara_context::{
    DateCwdReminder, InternalUrls, PromptTool, SystemPromptOptions, build_system_prompt, resolve_prompt_input,
};
use ara_discovery::{Discovery, HostDirs, ProviderPolicy, SkillsSettings};
use ara_mcp::ServerConfig as McpServerConfig;
use ara_session::{SessionJournal, latest_session};
use ara_tools::{ToolContext, builtin_tools};
use async_trait::async_trait;
use clap::{Parser, ValueEnum};
use std::io::{IsTerminal, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

mod proxy_discovery;

const TOOL_NAMES: [&str; 7] = ["read", "write", "edit", "bash", "grep", "glob", "ast_grep"];

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
    /// Discover this model's wire protocol from a dual-protocol proxy.
    #[value(name = "proxy-auto")]
    ProxyAuto,
}

impl Api {
    fn as_str(self) -> &'static str {
        match self {
            Self::AnthropicMessages => "anthropic-messages",
            Self::OpenaiCompletions => "openai-completions",
            Self::OpenaiResponses => "openai-responses",
            Self::ProxyAuto => "proxy-auto",
        }
    }
}

#[derive(Parser, Debug)]
#[command(name = "ara", version, about = "ARA agent (print mode, or a line REPL on a terminal)")]
struct Args {
    /// Prompts, sent in order. Piped stdin is prepended to the first prompt
    /// (or used alone when no prompt is given). With no prompt on a terminal,
    /// a line REPL starts instead.
    prompts: Vec<String>,
    /// Run the line REPL even when stdin is not a terminal, reading one
    /// prompt per line from stdin (scripted sessions and tests).
    #[arg(long, conflicts_with = "prompts")]
    repl: bool,
    /// Print mode (the only mode this host implements; accepted for OMP parity).
    #[arg(short = 'p', long)]
    print: bool,
    #[arg(long, value_enum, default_value_t = Mode::Text)]
    mode: Mode,
    /// Model id sent on the wire. Env: ARA_MODEL, ARA_TEST_MODEL_ID.
    #[arg(long)]
    model: Option<String>,
    /// Model wire protocol. proxy-auto probes an explicit dual-protocol proxy before starting a session.
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
    /// Auto-compact when estimated context exceeds this many tokens (0 disables).
    #[arg(long, default_value_t = 32_000)]
    compact_threshold: usize,
    /// Estimated recent tokens to keep raw when compacting.
    #[arg(long, default_value_t = 4_000)]
    compact_keep_tokens: usize,
    /// Output token cap for each model call; also caps the compaction summary
    /// budget. Must be at least 1.
    // Rejecting 0 here names the flag; accepted, it sent `max_tokens: 0` and
    // `/compact` failed later with an unexplained InvalidMaxTokens.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    max_tokens: Option<u64>,
    /// Confirm that the selected Responses model supports reasoning items.
    /// Requests encrypted reasoning for same-endpoint continuation.
    #[arg(long)]
    reasoning: bool,
    /// Store Responses on the provider and chain compatible turns in this process.
    /// Requires --api openai-responses; disabled by default.
    #[arg(long)]
    responses_stateful: bool,
    /// Request strict Anthropic tool schemas on a compatible custom endpoint.
    /// Requires --api anthropic-messages; official routes select strict automatically.
    #[arg(long)]
    anthropic_strict_tools: bool,
    /// Replay Chat reasoning_content on every assistant turn for routes that require it.
    /// Requires --api openai-completions; disabled by default.
    #[arg(long, conflicts_with = "chat_mistral_compat")]
    chat_replay_reasoning_content: bool,
    /// Use Mistral/Devstral Chat history fields on a compatible endpoint.
    /// Requires --api openai-completions; disabled by default.
    #[arg(long, conflicts_with = "chat_replay_reasoning_content")]
    chat_mistral_compat: bool,
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
    /// Tools to enable (comma separated): read,write,edit,bash,grep,glob,ast_grep. Empty disables tools.
    #[arg(long, default_value = "read,write,edit,bash,grep,glob")]
    tools: String,
    /// Explicit JSON config for one local MCP stdio server.
    #[arg(long)]
    mcp_config: Option<PathBuf>,
    /// Exact original MCP tool grant, as server:tool (repeatable).
    #[arg(long = "mcp-allow")]
    mcp_allow: Vec<String>,
    /// Prefix read output with line numbers.
    #[arg(long)]
    line_numbers: bool,
    /// Edit tool mode: hashline (default; anchored reads), replace, patch, apply_patch or sloppy.
    #[arg(long, default_value = "hashline", value_parser = parse_edit_mode)]
    edit_mode: ara_edit::EditMode,
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

fn parse_edit_mode(value: &str) -> Result<ara_edit::EditMode, String> {
    ara_edit::EditMode::parse(value)
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
    let encoded = format!("--{}--", cwd.to_string_lossy().trim_matches('/').replace(['/', '\\', ':', '?'], "-"));
    home.join("sessions").join(encoded)
}

/// Rewrite only a verbatim drive prefix (`\\?\C:\x` → `C:\x`) that Windows
/// `canonicalize` adds, as OMP's `setProjectDir` keeps a plain absolute path.
/// Verbatim UNC and device paths keep their spelling, so none turns relative.
fn plain_drive_path(path: PathBuf) -> PathBuf {
    let mut components = path.components();
    if let Some(Component::Prefix(prefix)) = components.next()
        && let std::path::Prefix::VerbatimDisk(letter) = prefix.kind()
    {
        let mut plain = PathBuf::from(format!("{}:", letter as char));
        plain.extend(components);
        return plain;
    }
    path
}

/// Writes JSON events and journals every completed message. A journal write
/// failure cancels the run so no further effect goes unrecorded.
struct HostSink {
    mode: Mode,
    /// REPL text mode: stream assistant text to stdout and report tool
    /// progress on stderr while the turn runs.
    stream: bool,
    /// A streamed text line is open on stdout (no trailing newline yet).
    line_open: AtomicBool,
    /// The current assistant message has streamed text.
    streamed: AtomicBool,
    journal: tokio::sync::Mutex<Option<SessionJournal>>,
    persist_failed: AtomicBool,
    /// Auto-compaction already said why it cannot run; it says so once per process.
    compaction_notice_shown: AtomicBool,
    cancel: CancellationToken,
    /// How `edit` payloads are written, to name their target files in progress lines.
    edit_mode: ara_edit::EditMode,
}

impl HostSink {
    fn new(
        mode: Mode,
        stream: bool,
        journal: Option<SessionJournal>,
        cancel: CancellationToken,
        edit_mode: ara_edit::EditMode,
    ) -> HostSink {
        HostSink {
            mode,
            stream,
            line_open: AtomicBool::new(false),
            streamed: AtomicBool::new(false),
            journal: tokio::sync::Mutex::new(journal),
            persist_failed: AtomicBool::new(false),
            compaction_notice_shown: AtomicBool::new(false),
            cancel,
            edit_mode,
        }
    }

    fn write_line(&self, line: &str) {
        let mut out = std::io::stdout().lock();
        // A closed stdout (e.g. `| head`) must not abort the run or the journal.
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    }

    /// Close a streamed text line, so the next output starts on its own line.
    fn end_stream_line(&self) {
        if self.line_open.swap(false, Ordering::SeqCst) {
            self.write_line("");
        }
    }

    fn stream_progress(&self, event: &AgentEvent) {
        match event {
            AgentEvent::MessageStart { .. } => self.streamed.store(false, Ordering::SeqCst),
            AgentEvent::MessageUpdate { event: AssistantMessageEvent::TextStart { .. }, .. } => self.end_stream_line(),
            AgentEvent::MessageUpdate { event: AssistantMessageEvent::TextDelta { delta, .. }, .. } => {
                let text = sanitize_text(delta);
                if !text.is_empty() {
                    let mut out = std::io::stdout().lock();
                    let _ = write!(out, "{text}");
                    let _ = out.flush();
                    self.line_open.store(!text.ends_with('\n'), Ordering::SeqCst);
                    self.streamed.store(true, Ordering::SeqCst);
                }
            }
            AgentEvent::MessageEnd { message: Message::Assistant(a) } => {
                self.end_stream_line();
                // A provider that delivered its text without deltas still shows it.
                if !self.streamed.swap(false, Ordering::SeqCst) {
                    for block in &a.content {
                        if let ara_ai::AssistantBlock::Text(t) = block
                            && !t.text.is_empty()
                        {
                            self.write_line(&sanitize_text(&t.text));
                        }
                    }
                }
            }
            AgentEvent::ToolExecutionStart { tool_name, args, .. } => {
                self.end_stream_line();
                eprintln!("ara: tool {}{}", sanitize_text(tool_name), tool_summary(tool_name, args, self.edit_mode));
            }
            AgentEvent::ToolExecutionEnd { tool_name, is_error, .. } => {
                eprintln!("ara: tool {} {}", sanitize_text(tool_name), if *is_error { "failed" } else { "done" });
            }
            _ => {}
        }
    }
}

/// One-line summary of a tool call for REPL progress: its command, path or
/// pattern, with whitespace folded and cut to 120 characters. An `edit` whose
/// payload is one `input` names its target file instead, as OMP's
/// `editToolRenderer.activitySummary` does.
fn tool_summary(tool_name: &str, args: &ara_ai::JsonObject, edit_mode: ara_edit::EditMode) -> String {
    let named = ["command", "path", "pattern", "pat"]
        .iter()
        .find_map(|k| args.get(*k).and_then(serde_json::Value::as_str))
        .map(str::to_owned);
    let edit_target = || {
        let input = args.get("input").and_then(serde_json::Value::as_str)?;
        (tool_name == "edit").then(|| edit_input_target(edit_mode, input)).flatten()
    };
    let Some(value) = named.or_else(edit_target) else {
        return String::new();
    };
    let flat = sanitize_text(&value).split_whitespace().collect::<Vec<_>>().join(" ");
    let cut: String = flat.chars().take(120).collect();
    if cut.len() < flat.len() { format!(": {cut}...") } else { format!(": {cut}") }
}

/// The first target file of an `edit` `input` payload, with `(+N more)` for
/// the rest (OMP `resolveEditCallFacts`). `None` when the payload does not
/// parse, so the progress line stays bare as before.
fn edit_input_target(mode: ara_edit::EditMode, input: &str) -> Option<String> {
    use ara_edit::EditMode;
    use ara_edit::modes::{apply_patch, hashline::input as hashline, sloppy};
    let paths: Vec<String> = match mode {
        EditMode::Hashline => hashline::Patch::parse(input, &hashline::SplitOptions::default())
            .ok()?
            .sections
            .into_iter()
            .map(|s| s.path)
            .collect(),
        EditMode::ApplyPatch => {
            apply_patch::parse_apply_patch_streaming(input).ok()?.into_iter().map(|e| e.path).collect()
        }
        EditMode::Sloppy => sloppy::parse::split_sloppy_sections(input).into_iter().map(|s| s.path).collect(),
        EditMode::Replace | EditMode::Patch => return None,
    };
    let first = paths.first()?;
    Some(if paths.len() > 1 { format!("{first} (+{} more)", paths.len() - 1) } else { first.clone() })
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
        if self.stream {
            self.stream_progress(&event);
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
    if args.api == Api::ProxyAuto {
        if args.resume.is_some() || args.continue_session {
            bail!("--api proxy-auto cannot resume a session; select an explicit --api and verify its endpoint");
        }
        if args.reasoning
            || args.responses_stateful
            || args.anthropic_strict_tools
            || args.chat_replay_reasoning_content
            || args.chat_mistral_compat
            || args.report_request_text_tokens
        {
            bail!("--api proxy-auto cannot use protocol-specific flags; select an explicit --api");
        }
        if matches!(args.provider.as_deref(), Some("opencode-go" | "opencode-zen" | "umans")) {
            bail!("--api proxy-auto cannot use a provider label with a different authentication scheme");
        }
    }
    if args.reasoning && args.api != Api::OpenaiResponses {
        bail!("--reasoning requires --api openai-responses");
    }
    if args.responses_stateful && args.api != Api::OpenaiResponses {
        bail!("--responses-stateful requires --api openai-responses");
    }
    if args.anthropic_strict_tools && args.api != Api::AnthropicMessages {
        bail!("--anthropic-strict-tools requires --api anthropic-messages");
    }
    if args.chat_replay_reasoning_content && args.api != Api::OpenaiCompletions {
        bail!("--chat-replay-reasoning-content requires --api openai-completions");
    }
    if args.chat_mistral_compat && args.api != Api::OpenaiCompletions {
        bail!("--chat-mistral-compat requires --api openai-completions");
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
    if args.api == Api::ProxyAuto && is_official_anthropic(&base_url) {
        bail!("--api proxy-auto requires a dual-protocol proxy, not the official Anthropic route");
    }
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
        } else if args.api == Api::ProxyAuto {
            "proxy".into()
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
    stream_options.compat.requires_reasoning_content_on_all_assistant_turns = args.chat_replay_reasoning_content;
    if args.chat_mistral_compat {
        stream_options.compat.requires_mistral_tool_ids = true;
        stream_options.compat.requires_tool_result_name = true;
        stream_options.compat.requires_assistant_after_tool_result = true;
        stream_options.compat.requires_thinking_as_text = true;
    }
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

/// V1-COMPACT: single-level soft compaction from the A3 pieces. The summary
/// is persisted in the Session with its source entry IDs; raw history stays
/// in the journal and resume reads the projected context. A second compaction
/// on an already-compacted session is refused: chained summaries need the
/// still-open A3 persistence decision, so V1 keeps one level.
#[allow(clippy::too_many_arguments)]
async fn run_compaction(
    args: &Args,
    model: &ara_ai::Model,
    provider: &Arc<dyn ModelProvider>,
    context: &mut Vec<Message>,
    sink: &HostSink,
    cancel: &CancellationToken,
    manual: bool,
) -> Result<bool> {
    use ara_agent::compaction::{
        SummaryInputError, SummarySource, explain_no_whole_turn_cut, select_whole_turn_cut, summarize_sources,
    };
    use ara_agent::tokenizer::{MessageCountOptions, count_message, count_messages};
    use ara_session::CompactionSourceError;
    let tokens = count_messages(context, MessageCountOptions::default());
    if !manual && (args.compact_threshold == 0 || tokens <= args.compact_threshold) {
        return Ok(false);
    }
    // A manual /compact always explains a refusal; auto-compaction explains
    // it once, not after every turn.
    let notice = |text: String| {
        if manual || !sink.compaction_notice_shown.swap(true, Ordering::SeqCst) {
            let prefix = if manual { "" } else { "auto-" };
            eprintln!("ara: {prefix}compaction skipped: {text}");
        }
    };
    let mut guard = sink.journal.lock().await;
    let Some(journal) = guard.as_mut() else {
        notice("it needs a session (--no-session is set)".into());
        return Ok(false);
    };
    let snapshot = match journal.compaction_source_snapshot() {
        Ok(s) => s,
        Err(CompactionSourceError::UnsupportedContextEntry { kind, .. }) if kind == "compaction" => {
            // An entry the Session cannot project is not a usable compaction.
            match journal.compacted_context_projection() {
                Ok(_) => notice("this session is already compacted, and V1 keeps a single level per session".into()),
                Err(e) => notice(format!("the session has a compaction entry it cannot use ({e})")),
            }
            return Ok(false);
        }
        Err(e) => {
            notice(format!("the session source is unavailable ({e})"));
            return Ok(false);
        }
    };
    let sources: Vec<SummarySource<'_>> = snapshot
        .messages
        .iter()
        .map(|m| SummarySource { entry_id: m.entry_id.as_str(), message: &m.message })
        .collect();
    let Some(cut) =
        select_whole_turn_cut(&sources, args.compact_keep_tokens, None).map_err(|e| anyhow::anyhow!("{e}"))?
    else {
        let raw: usize = sources.iter().map(|s| count_message(s.message, MessageCountOptions::default())).sum();
        match explain_no_whole_turn_cut(&sources) {
            _ if raw <= args.compact_keep_tokens => {
                if manual {
                    eprintln!("ara: history is already small; nothing to compact");
                }
            }
            // Not expected: a cut exists but none was selected above the target.
            None => notice("no cut was selected".into()),
            Some(SummaryInputError::EmptySources) => notice("there is no earlier turn to summarize".into()),
            Some(e) => notice(format!("no earlier turn can be summarized ({e})")),
        }
        return Ok(false);
    };
    let span = &sources[..cut.candidate.first_kept_index];
    let deadline = Instant::now() + Duration::from_secs(120);
    // OMP's default summary budget, floor(0.8 * reserveTokens) with the default
    // 16384 reserve; ARA knows no context window, so it keeps the default.
    // A reasoning model spends part of it on thinking. `--max-tokens` caps it.
    const SUMMARY_MAX_TOKENS: u64 = 16_384 * 4 / 5;
    let max_tokens = args.max_tokens.map_or(SUMMARY_MAX_TOKENS, |cap| cap.min(SUMMARY_MAX_TOKENS));
    let accepted = match summarize_sources(span, None, model, provider.as_ref(), max_tokens, deadline, cancel).await {
        Ok(a) => a,
        Err(e) => {
            // Show the stop reason, output tokens and the provider's status and
            // message so the cause is visible.
            let stop = e.stop_reason.map(|r| format!(", stop reason {}", r.as_str())).unwrap_or_default();
            let output = e
                .usage
                .as_ref()
                .and_then(|u| u.output)
                .map(|n| format!(", {n} of {max_tokens} output tokens"))
                .unwrap_or_default();
            let status = e.provider_status.map(|s| format!(", HTTP {s}")).unwrap_or_default();
            let detail = e.provider_message.as_deref().map(|m| format!(": {}", sanitize_text(m))).unwrap_or_default();
            eprintln!("ara: summary call failed ({e}{stop}{output}{status}{detail}); session untouched");
            return Ok(false);
        }
    };
    journal.append_compaction(
        &accepted.text,
        &cut.candidate.first_kept_entry_id,
        &accepted.window_source_entry_ids,
        tokens as u64,
    )?;
    // `model_context` falls back to the raw history when the Session cannot
    // project the summary, so check it rather than claim a compaction.
    if let Err(e) = journal.compacted_context_projection() {
        eprintln!("ara: compaction entry written, but the session cannot use it ({e}); context unchanged");
        return Ok(false);
    }
    *context = journal.model_context();
    let kept = count_messages(context, MessageCountOptions::default());
    eprintln!("ara: compacted {tokens} estimated tokens down to {kept}; summary persisted with source IDs");
    Ok(true)
}

/// Ctrl+C, plus Ctrl+Break on Windows (the console event another process can
/// deliver to a separate process group), as one stream of interrupts.
type Interrupts = tokio::sync::mpsc::UnboundedReceiver<()>;

fn listen_for_interrupts() -> Result<Interrupts> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigint = signal(SignalKind::interrupt()).context("installing the SIGINT handler")?;
        tokio::spawn(async move { while sigint.recv().await.is_some() && tx.send(()).is_ok() {} });
    }
    #[cfg(windows)]
    {
        use tokio::signal::windows::{ctrl_break, ctrl_c};
        let mut c = ctrl_c().context("installing the Ctrl+C handler")?;
        let mut b = ctrl_break().context("installing the Ctrl+Break handler")?;
        tokio::spawn(async move {
            loop {
                let got = tokio::select! { v = c.recv() => v, v = b.recv() => v };
                if got.is_none() || tx.send(()).is_err() {
                    break;
                }
            }
        });
    }
    Ok(rx)
}

/// Drive one REPL step (a turn or a compaction). The first interrupt cancels
/// `token` so the step winds down and records its own abort; a second one
/// exits at once with 130, as in print mode.
async fn interruptible<F: Future>(step: F, token: &CancellationToken, interrupts: &mut Interrupts) -> F::Output {
    tokio::pin!(step);
    let mut listening = true;
    loop {
        tokio::select! {
            out = &mut step => return out,
            got = interrupts.recv(), if listening => match got {
                None => listening = false,
                Some(()) if token.is_cancelled() => {
                    eprintln!("ara: second interrupt, exiting");
                    std::process::exit(130);
                }
                Some(()) => {
                    eprintln!("ara: interrupt received, aborting (press Ctrl-C again to exit immediately)");
                    token.cancel();
                }
            },
        }
    }
}

/// Where the REPL keeps its Session, for `/new`.
struct ReplSession<'a> {
    /// `None` under `--no-session`.
    dir: Option<PathBuf>,
    cwd: &'a Path,
    model_ref: &'a str,
}

/// V1-REPL + V1-CANCEL: line-based session, one turn per input line in the
/// same Session journal. In text mode the answer streams to stdout and each
/// tool call is reported on stderr as it starts and ends. Ctrl+C during a turn cancels only that turn (its
/// child token) and returns to the prompt; Ctrl+C at the idle prompt exits
/// with 130; EOF and `/exit` exit with 0, and an unreadable line exits with 1. `/new` starts a fresh Session file
/// and keeps the previous one intact.
#[allow(clippy::too_many_arguments)]
async fn run_repl_loop(
    args: &Args,
    model: &ara_ai::Model,
    provider: &Arc<dyn ModelProvider>,
    system_prompt: &[String],
    tools: &[Arc<dyn ara_agent::AgentTool>],
    hooks: &Arc<dyn LoopHooks>,
    context: &mut Vec<Message>,
    sink: &HostSink,
    cancel: &CancellationToken,
    session: ReplSession<'_>,
    mut interrupts: Interrupts,
) -> Result<i32> {
    eprintln!("ara: interactive session ({}). /help for commands.", model.id);
    let current_path = || async { sink.journal.lock().await.as_ref().map(|j| j.path().to_path_buf()) };
    if let Some(path) = current_path().await
        && path.exists()
    {
        eprintln!("ara: session {}", path.display());
    }
    let mut turn: usize = 0;
    let code = loop {
        eprint!("> ");
        let _ = std::io::stderr().flush();
        let read = tokio::task::spawn_blocking(|| {
            let mut buf = String::new();
            match std::io::stdin().read_line(&mut buf) {
                Ok(0) => Ok(None),
                Ok(_) => Ok(Some(buf)),
                Err(e) => Err(e.to_string()),
            }
        });
        let line = tokio::select! {
            line = read => line.unwrap_or_else(|e| Err(e.to_string())),
            Some(()) = interrupts.recv() => {
                eprintln!();
                break 130;
            }
        };
        let raw = match line {
            Ok(Some(raw)) => raw,
            Ok(None) => {
                eprintln!();
                break 0;
            }
            // For example input that is not UTF-8: say so instead of a silent exit 0.
            Err(e) => {
                eprintln!();
                eprintln!("ara: cannot read the next prompt ({e}); session kept");
                break 1;
            }
        };
        let input = sanitize_text(raw.trim());
        if input.is_empty() {
            continue;
        }
        match input.as_str() {
            "/exit" | "/quit" => break 0,
            "/help" => {
                eprintln!("commands: /help, /new, /compact, /exit");
                eprintln!("Ctrl+C during a turn cancels it; Ctrl+C at the prompt exits.");
                continue;
            }
            "/compact" => {
                let token = cancel.child_token();
                let step = run_compaction(args, model, provider, context, sink, &token, true);
                if let Err(e) = interruptible(step, &token, &mut interrupts).await {
                    eprintln!("ara: compaction failed ({e:#}); session kept");
                }
                continue;
            }
            "/new" => {
                let mut guard = sink.journal.lock().await;
                match (guard.as_mut(), &session.dir) {
                    (Some(journal), Some(dir)) => {
                        let fresh = SessionJournal::create(dir, session.cwd).and_then(|mut j| {
                            j.append_model_change(session.model_ref)?;
                            Ok(j)
                        });
                        match fresh {
                            Ok(fresh) => {
                                let previous = std::mem::replace(journal, fresh);
                                if args.mode == Mode::Json {
                                    sink.write_line(&journal.header().to_string());
                                }
                                if previous.path().exists() {
                                    eprintln!(
                                        "ara: new session; the previous one stays at {}",
                                        previous.path().display()
                                    );
                                } else {
                                    eprintln!("ara: new session");
                                }
                            }
                            Err(e) => {
                                eprintln!("ara: could not start a new session ({e:#}); keeping the current one");
                                continue;
                            }
                        }
                    }
                    _ => eprintln!("ara: new conversation (not persisted)"),
                }
                context.clear();
                turn = 0;
                continue;
            }
            _ => {}
        }
        turn += 1;
        eprintln!("Working... (turn {turn})");
        let config = AgentConfig {
            model: model.clone(),
            provider: provider.clone(),
            system_prompt: system_prompt.to_vec(),
            tools: tools.to_vec(),
            tool_choice: None,
            max_tokens: args.max_tokens,
            temperature: args.temperature,
            deadline: args.max_time.map(|s| Instant::now() + Duration::from_secs_f64(s.max(0.0))),
            max_model_calls: args.max_model_calls,
            hooks: hooks.clone(),
        };
        let turn_start = context.len();
        let token = cancel.child_token();
        let step = agent_loop(vec![Message::User(UserMessage::text(input))], context, &config, &token, sink);
        let report = interruptible(step, &token, &mut interrupts).await;
        // Text mode streamed the answer while the turn ran.
        sink.end_stream_line();
        match report.end {
            RunEnd::Completed => {
                if let Some(e) =
                    context.iter().rev().find_map(Message::as_assistant).and_then(|a| a.error_message.as_ref())
                {
                    eprintln!("{}", sanitize_text(e));
                }
            }
            RunEnd::Aborted => eprintln!("ara: turn {turn} cancelled; session kept, type the next prompt"),
            RunEnd::Deadline => eprintln!("ara: turn {turn} hit the deadline; session kept"),
            RunEnd::ModelCallBudget => {
                eprintln!("ara: model call limit reached ({})", args.max_model_calls.unwrap_or_default())
            }
            RunEnd::Error => {
                // Show this turn's provider cause, as print mode does.
                let cause = context
                    .get(turn_start..)
                    .unwrap_or_default()
                    .iter()
                    .rev()
                    .find_map(Message::as_assistant)
                    .filter(|a| matches!(a.stop_reason, StopReason::Error))
                    .and_then(|a| a.error_message.as_deref())
                    .map(|m| format!(" ({})", sanitize_text(m)))
                    .unwrap_or_default();
                eprintln!("ara: turn {turn} ended in error{cause}; session kept")
            }
        }
        if sink.persist_failed.load(Ordering::SeqCst) {
            break 1;
        }
        if report.end == RunEnd::Completed {
            let token = cancel.child_token();
            let step = run_compaction(args, model, provider, context, sink, &token, false);
            if let Err(e) = interruptible(step, &token, &mut interrupts).await {
                eprintln!("ara: auto-compaction failed ({e:#}); session kept");
            }
        }
    };
    if let Some(path) = current_path().await
        && path.exists()
    {
        eprintln!("ara: session {}", path.display());
    }
    Ok(code)
}

async fn run(args: Args) -> Result<i32> {
    let _ = args.print;
    let mut route = resolve_route(&args)?;
    if args.mcp_config.is_none() && !args.mcp_allow.is_empty() {
        bail!("--mcp-allow requires --mcp-config");
    }
    let mcp_config =
        args.mcp_config.as_deref().map(McpServerConfig::from_file).transpose().map_err(anyhow::Error::msg)?;
    let mut prompts = args.prompts.clone();
    // Under `--repl`, stdin is the REPL's line input, not a prompt.
    if !args.repl
        && let Some(stdin) = read_stdin()?
    {
        // OMP buildInitialMessage: `${stdin}\n${firstPrompt}`.
        match prompts.first_mut() {
            Some(first) => *first = format!("{stdin}\n{first}"),
            None => prompts.push(stdin),
        }
    }
    let repl_mode = args.repl || (prompts.is_empty() && std::io::stdin().is_terminal());
    if prompts.is_empty() && !repl_mode {
        bail!("no prompt given (pass it as an argument or on stdin)");
    }
    let explicit_cwd = match &args.cwd {
        Some(c) => {
            let canonical = std::fs::canonicalize(c).with_context(|| format!("--cwd {}", c.display()))?;
            Some(plain_drive_path(canonical))
        }
        None => None,
    };
    let launch_cwd = explicit_cwd.clone().map(Ok).unwrap_or_else(std::env::current_dir)?;

    let selected_api = if args.api == Api::ProxyAuto {
        let api = proxy_discovery::discover_proxy_api(
            &route.model.base_url,
            &route.model.id,
            route.stream_options.api_key.as_deref(),
            &route.stream_options.extra_headers,
        )
        .await?;
        route.model.api = api.as_str().into();
        api
    } else {
        args.api
    };

    // Session journal (first journal I/O happens only after validation above).
    let mut cwd = launch_cwd.clone();
    let session_dir =
        (!args.no_session).then(|| args.session_dir.clone().unwrap_or_else(|| default_session_dir(&launch_cwd)));
    let mut journal = if let Some(dir) = &session_dir {
        let path = match (&args.resume, args.continue_session) {
            (Some(p), _) => Some(p.clone()),
            (None, true) => {
                Some(latest_session(dir).with_context(|| format!("no session to continue in {}", dir.display()))?)
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
            None => SessionJournal::create(dir, &cwd)?,
        })
    } else {
        None
    };
    let model_ref = format!("{}/{}", route.model.provider, route.model.id);
    let mut context: Vec<Message> = journal.as_ref().map(SessionJournal::model_context).unwrap_or_default();
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
    let ast_ctx = tool_ctx.clone();
    let mut tools: Vec<_> =
        builtin_tools(tool_ctx).into_iter().filter(|t| enabled.contains(&t.definition().name.as_str())).collect();
    if enabled.contains(&"ast_grep") {
        tools.push(Arc::new(ara_tools::ast_grep::AstGrepTool { ctx: ast_ctx }));
    }
    if let Some(config) = mcp_config {
        let mcp_tools =
            ara_mcp::connect(config, &cwd, &args.mcp_allow, &TOOL_NAMES).await.map_err(anyhow::Error::msg)?;
        tools.extend(mcp_tools);
    }

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
    let mut client_builder = reqwest::Client::builder();
    if args.api == Api::ProxyAuto {
        client_builder = client_builder.redirect(reqwest::redirect::Policy::none());
    }
    let client = client_builder.build().context("building HTTP client")?;
    let provider: Arc<dyn ModelProvider> = match selected_api {
        Api::AnthropicMessages => Arc::new(AnthropicMessagesProvider {
            client,
            base: ara_ai::providers::anthropic::StreamOptions {
                api_key: stream_options.api_key,
                strict_tools: args.anthropic_strict_tools.then_some(true),
                provider_session_state: Some(Arc::new(Default::default())),
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
        Api::ProxyAuto => unreachable!("proxy discovery resolves to a concrete protocol"),
    };
    let cancel = CancellationToken::new();
    let sink = HostSink::new(args.mode, repl_mode && args.mode == Mode::Text, journal, cancel.clone(), args.edit_mode);
    if args.mode == Mode::Json {
        sink.write_line(&header.to_string());
    }

    let mut interrupts = listen_for_interrupts()?;
    if repl_mode {
        let session = ReplSession { dir: session_dir, cwd: &cwd, model_ref: &model_ref };
        return run_repl_loop(
            &args,
            &route.model,
            &provider,
            &system_prompt,
            &tools,
            &hooks,
            &mut context,
            &sink,
            &cancel,
            session,
            interrupts,
        )
        .await;
    }
    let c2 = cancel.clone();
    tokio::spawn(async move {
        if interrupts.recv().await.is_some() {
            eprintln!("ara: interrupt received, aborting (press Ctrl-C again to exit immediately)");
            c2.cancel();
            if interrupts.recv().await.is_some() {
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
        if let Err(e) = run_compaction(&args, &route.model, &provider, &mut context, &sink, &cancel, false).await {
            eprintln!("ara: auto-compaction failed ({e:#}); session kept");
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
    fn edit_progress_names_the_target_file() {
        use ara_edit::EditMode;
        use serde_json::json;
        let edit = |input: &str, mode: EditMode| {
            let args = json!({ "input": input }).as_object().cloned().unwrap();
            tool_summary("edit", &args, mode)
        };
        let one = "[docs/a.md#1A2B]
PUT 3.=3:
+x
";
        assert_eq!(edit(one, EditMode::Hashline), ": docs/a.md");
        let two = "[src/a.rs#1A2B]
PUT 1.=1:
+x
[src/b.rs#3C4D]
PUT 2.=2:
+y
";
        assert_eq!(edit(two, EditMode::Hashline), ": src/a.rs (+1 more)");
        let codex = "*** Begin Patch
*** Update File: src/x.rs
@@
-a
+b
*** End Patch";
        assert_eq!(edit(codex, EditMode::ApplyPatch), ": src/x.rs");
        let sloppy = "<SM:EDIT path=\"src/s.ts\">
<SM:FIND>
const x = 1;
</SM:FIND>
<SM:PUT>
const x = 2;
</SM:PUT>";
        assert_eq!(edit(sloppy, EditMode::Sloppy), ": src/s.ts");
        // A payload that does not parse, and an `input` on another tool, stay bare.
        assert_eq!(edit("no header", EditMode::Hashline), "");
        let args = json!({ "input": one }).as_object().cloned().unwrap();
        assert_eq!(tool_summary("mcp_tool", &args, EditMode::Hashline), "");
        // Modes with a `path` argument keep showing it.
        let args = json!({ "path": "src/p.rs", "edits": [] }).as_object().cloned().unwrap();
        assert_eq!(tool_summary("edit", &args, EditMode::Patch), ": src/p.rs");
    }

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
        let sink = HostSink::new(Mode::Text, false, Some(j), CancellationToken::new(), ara_edit::EditMode::Hashline);
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
