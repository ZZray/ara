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

mod skill_command;
use ara_ai::providers::openai_completions::{PreparedRequestTextObservation, RequestTextObserver, StreamOptions};
use ara_ai::providers::openai_responses::StreamOptions as ResponsesStreamOptions;
use ara_ai::{AssistantMessageEvent, Message, Model, ModelProvider, ModelTokenizer, StopReason, UserMessage};
use ara_context::{
    DateCwdReminder, InternalUrls, PromptTool, SystemPromptOptions, build_system_prompt, resolve_prompt_input,
};
use ara_discovery::{Discovery, HostDirs, ProviderPolicy, SkillSourceSwitches, SkillsSettings};
use ara_mcp::ServerConfig as McpServerConfig;
use ara_session::{SessionJournal, latest_session};
use ara_tools::{ToolContext, builtin_tools};
use async_trait::async_trait;
use clap::{Parser, Subcommand, ValueEnum};
use std::io::{IsTerminal, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

mod proxy_discovery;
mod rpc_host;
mod rpc_host_retry;
mod rpc_host_settings;
mod rpc_host_tools;
mod rpc_host_uris;

const TOOL_NAMES: [&str; 7] = ["read", "write", "edit", "bash", "grep", "glob", "ast_grep"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Mode {
    /// Final assistant text only.
    Text,
    /// Every agent event as one JSON line.
    Json,
    /// JSONL RPC commands and live Agent events.
    Rpc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Api {
    #[value(name = "anthropic-messages")]
    AnthropicMessages,
    #[value(name = "openai-completions")]
    OpenaiCompletions,
    #[value(name = "openai-responses")]
    OpenaiResponses,
    #[value(name = "openai-codex-responses")]
    OpenaiCodexResponses,
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
            Self::OpenaiCodexResponses => "openai-codex-responses",
            Self::ProxyAuto => "proxy-auto",
        }
    }
}

#[derive(Parser, Debug, Clone)]
#[command(name = "ara", version, about = "ARA agent (print mode, or a line REPL on a terminal)")]
struct Args {
    #[command(subcommand)]
    command: Option<AuthCommand>,
    /// Prompts, sent in order. Piped stdin is prepended to the first prompt
    /// (or used alone when no prompt is given). With no prompt on a terminal,
    /// a line REPL starts instead.
    prompts: Vec<String>,
    /// Run the line REPL even when stdin is not a terminal, reading one
    /// prompt per line from stdin (scripted sessions and tests).
    #[arg(long, conflicts_with = "prompts")]
    repl: bool,
    /// Run print mode (accepted for OMP parity).
    #[arg(short = 'p', long)]
    print: bool,
    #[arg(long, value_enum, default_value_t = Mode::Text)]
    mode: Mode,
    /// Model id sent on the wire. Env: ARA_MODEL, ARA_TEST_MODEL_ID.
    #[arg(long)]
    model: Option<String>,
    /// Model wire protocol. proxy-auto probes an explicit dual-protocol proxy before starting a session.
    #[arg(long, value_enum)]
    api: Option<Api>,
    /// Models configuration file (default: $ARA_HOME/agent/models.yml).
    #[arg(long)]
    models_config: Option<PathBuf>,
    /// Local catalog content tokenizer: auto, none, claude-v3, claude-v47,
    /// claude-v5, claude-v5-sonnet, qwen3, deepseek-v3, kimi-k2 or glm5. Env: ARA_TOKENIZER.
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
    /// Compatible Skill sources to discover at both user and project levels
    /// (comma separated): agents,claude,codex,opencode. Empty disables these four.
    #[arg(long, default_value = "agents,claude,codex")]
    skill_sources: SkillSourceSwitches,
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

impl Args {
    fn api(&self) -> Api {
        self.api.unwrap_or(Api::OpenaiCompletions)
    }
}

#[derive(Subcommand, Debug, Clone)]
enum AuthCommand {
    /// Sign in to OpenAI using a device code.
    Login {
        #[arg(default_value = "openai-codex")]
        provider: String,
    },
    /// Remove the saved OpenAI account credentials.
    Logout {
        #[arg(default_value = "openai-codex")]
        provider: String,
    },
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
        .or_else(|| {
            std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(|h| PathBuf::from(h).join(".ara"))
        })
        .unwrap_or_else(|| PathBuf::from(".ara"));
    std::path::absolute(&home).unwrap_or(home)
}

/// Fixed utils/dirs.ts getBlobsDir, under the reference Host's agent data root.
fn ara_blobs_directory() -> PathBuf {
    ara_home().join("agent").join("blobs")
}

fn user_home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
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
    artifacts: Arc<ara_cli::session_artifacts::ArtifactUriRouter>,
    persist_failed: AtomicBool,
    /// Auto-compaction already said why it cannot run; it says so once per process.
    compaction_notice_shown: AtomicBool,
    cancel: CancellationToken,
    /// How `edit` payloads are written, to name their target files in progress lines.
    edit_mode: ara_edit::EditMode,
    observed_usage: Option<Arc<ara_cli::auth_storage::AuthStorage>>,
}

impl HostSink {
    fn persistence_failure(&self, error: &impl std::fmt::Display) {
        self.persist_failed.store(true, Ordering::SeqCst);
        eprintln!("ara: session persistence failed ({error}); aborting the run");
        self.cancel.cancel();
    }

    fn new(
        mode: Mode,
        stream: bool,
        journal: Option<SessionJournal>,
        cancel: CancellationToken,
        edit_mode: ara_edit::EditMode,
        artifacts: Arc<ara_cli::session_artifacts::ArtifactUriRouter>,
    ) -> HostSink {
        HostSink {
            mode,
            stream,
            line_open: AtomicBool::new(false),
            streamed: AtomicBool::new(false),
            journal: tokio::sync::Mutex::new(journal),
            artifacts,
            persist_failed: AtomicBool::new(false),
            compaction_notice_shown: AtomicBool::new(false),
            cancel,
            edit_mode,
            observed_usage: None,
        }
    }

    fn with_observed_usage(mut self, owner: Option<Arc<ara_cli::auth_storage::AuthStorage>>) -> Self {
        self.observed_usage = owner;
        self
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
            self.persistence_failure(&e);
        }
        if let AgentEvent::MessageEnd { message: Message::Assistant(message) } = &event
            && let Some(owner) = &self.observed_usage
            && let (Some(input), Some(output), Some(cache_read), Some(cache_write), Some(cost)) = (
                message.usage.input,
                message.usage.output,
                message.usage.cache_read,
                message.usage.cache_write,
                message.usage.cost.as_ref(),
            )
            && cost.total.is_finite()
            && let (Ok(input_tokens), Ok(output_tokens), Ok(cache_read_tokens), Ok(cache_write_tokens)) =
                (i64::try_from(input), i64::try_from(output), i64::try_from(cache_read), i64::try_from(cache_write))
        {
            owner.record_observed_usage(
                &[ara_cli::credential_store::ClientUsageEntry {
                    at: message.timestamp,
                    provider: message.provider.clone(),
                    model: message.model.clone(),
                    requests: 1,
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    cache_write_tokens,
                    cost_usd: cost.total,
                }],
                None,
            );
        }
        // Incomplete usage remains unknown in the journal; the fixed numeric
        // broker report cannot represent missing buckets or unknown cost.
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
    daily: Option<ara_cli::daily_model_config::DailySelection>,
    registry: Option<ara_cli::model_registry::ModelRegistry>,
}

/// Validate every argument that does not need the journal (no I/O side effects).
fn validate_route_args(args: &Args) -> Result<()> {
    if args.api() == Api::ProxyAuto {
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
    if args.reasoning && !matches!(args.api(), Api::OpenaiResponses | Api::OpenaiCodexResponses) {
        bail!("--reasoning requires --api openai-responses or openai-codex-responses");
    }
    if args.responses_stateful && args.api() != Api::OpenaiResponses {
        bail!("--responses-stateful requires --api openai-responses");
    }
    if args.anthropic_strict_tools && args.api() != Api::AnthropicMessages {
        bail!("--anthropic-strict-tools requires --api anthropic-messages");
    }
    if args.chat_replay_reasoning_content && args.api() != Api::OpenaiCompletions {
        bail!("--chat-replay-reasoning-content requires --api openai-completions");
    }
    if args.chat_mistral_compat && args.api() != Api::OpenaiCompletions {
        bail!("--chat-mistral-compat requires --api openai-completions");
    }
    if args.api() != Api::OpenaiCompletions && args.report_request_text_tokens {
        bail!("--report-request-text-tokens requires --api openai-completions");
    }
    for name in args.tools.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if !TOOL_NAMES.contains(&name) {
            bail!("unknown tool {name:?} (available: {})", TOOL_NAMES.join(", "));
        }
    }
    Ok(())
}

fn resolve_route(args: &Args) -> Result<Route> {
    validate_route_args(args)?;
    let model_id = args
        .model
        .clone()
        .or_else(|| env_first(&["ARA_MODEL", "ARA_TEST_MODEL_ID"]).map(|(_, v)| v))
        .context("no model: pass --model or set ARA_MODEL")?;
    let tokenizer_choice = args.tokenizer.clone().or_else(|| env_first(&["ARA_TOKENIZER"]).map(|(_, value)| value));
    let tokenizer = match tokenizer_choice.as_deref() {
        None | Some("auto") =>
            ara_cli::model_policy::resolve_model_tokenizer(&model_id)?.and_then(ModelTokenizer::from_name),
        Some("none") => None,
        Some(name) => Some(ModelTokenizer::from_name(name).with_context(|| {
            format!("unknown tokenizer {name:?}; use auto, none, claude-v3, claude-v47, claude-v5, claude-v5-sonnet, qwen3, deepseek-v3, kimi-k2 or glm5")
        })?),
    };
    let base_url = args
        .base_url
        .clone()
        .or_else(|| env_first(&["ARA_BASE_URL", "ARA_TEST_BASE_URL", "OPENROUTER_BASE_URL"]).map(|(_, v)| v))
        .context("no base URL: pass --base-url or set ARA_BASE_URL")?;
    if args.api() == Api::ProxyAuto && is_official_anthropic(&base_url) {
        bail!("--api proxy-auto requires a dual-protocol proxy, not the official Anthropic route");
    }
    let openrouter = is_openrouter(&base_url);
    // Route-specific keys stay on their own hosts. ARA keys or an explicit
    // --api-key-env may be used for another endpoint.
    let api_key = match &args.api_key_env {
        Some(name) => Some(std::env::var(name).with_context(|| format!("environment variable {name} is not set"))?),
        None if args.api() == Api::AnthropicMessages && is_official_anthropic(&base_url) => {
            env_first(&["ANTHROPIC_API_KEY", "ARA_API_KEY", "ARA_TEST_API_KEY"]).map(|(_, v)| v)
        }
        None if args.api() == Api::AnthropicMessages => env_first(&["ARA_API_KEY", "ARA_TEST_API_KEY"]).map(|(_, v)| v),
        None if openrouter => env_first(&["OPENROUTER_API_KEY"]).map(|(_, v)| v),
        None => env_first(&["ARA_API_KEY", "ARA_TEST_API_KEY"]).map(|(_, v)| v),
    };
    let provider = args.provider.clone().unwrap_or_else(|| {
        if args.api() == Api::AnthropicMessages {
            "anthropic".into()
        } else if openrouter {
            "openrouter".into()
        } else if args.api() == Api::ProxyAuto {
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
            api: args.api().as_str().into(),
            provider,
            base_url,
            reasoning: args.reasoning,
            max_tokens: None,
            context_window: None,
            tokenizer,
        },
        stream_options,
        daily: None,
        registry: None,
    })
}

async fn resolve_startup_route(
    args: &mut Args,
    project_dir: &Path,
    account_auth: &mut Option<Arc<ara_cli::auth_storage::AuthStorage>>,
    registry_cancel: &CancellationToken,
) -> Result<Route> {
    use ara_cli::daily_model_config::{
        DailyOverrides, load_daily_config, resolve_daily_selection_with_registry, validate_daily_auth_ownership,
    };
    use ara_cli::model_config_values::{ConfigValueEnvironment, ProcessConfigEnvironment};
    let mut host = ara_cli::model_registry::ModelRegistryHost::for_process(
        project_dir.to_path_buf(),
        &ara_home().join("agent/model-cache.db"),
    )?;
    host.cancel = registry_cancel.clone();
    let key_resolver = Arc::new(ara_cli::config_request_auth::ConfigStorageKeyResolver::new(
        project_dir.to_path_buf(),
        host.config_values.clone(),
        host.config_environment.clone(),
    ));
    let discovery_host =
        ara_cli::auth_broker_discover::BrokerDiscoveryHost::for_process(ara_home(), Some(key_resolver.clone()));
    let broker_config = ara_cli::auth_broker_discover::BrokerConfigResolver::new(discovery_host.clone())
        .resolve(registry_cancel)
        .await?;
    if broker_config.is_none()
        && args.models_config.is_none()
        && matches!(args.api(), Api::AnthropicMessages | Api::ProxyAuto)
    {
        return resolve_route(args);
    }
    let path = args.models_config.clone().unwrap_or_else(|| ara_home().join("agent/models.yml"));
    let config = load_daily_config(&path, args.models_config.is_some())?;
    let configured_provider =
        args.provider.as_deref().map(str::to_owned).or_else(|| std::env::var("ARA_PROVIDER").ok());
    if args.mode == Mode::Rpc
        && (args.api() == Api::OpenaiCodexResponses || configured_provider.as_deref() == Some("openai-codex"))
    {
        bail!("OpenAI account mode currently supports print and REPL; use --mode text or json");
    }
    if config.is_none()
        && broker_config.is_none()
        && args.api() != Api::OpenaiCodexResponses
        && configured_provider.as_deref() != Some("openai-codex")
        && !ara_home().join("agent/auth.db").exists()
    {
        return resolve_route(args);
    }
    let headers = args
        .headers
        .iter()
        .map(|header| {
            let (name, value) = header.split_once(':').context("--header must be Name: value")?;
            Ok((name.trim().to_owned(), value.trim().to_owned()))
        })
        .collect::<Result<Vec<_>>>()?;
    let overrides = DailyOverrides {
        provider: args.provider.clone(),
        model: args.model.clone(),
        api: args.api.map(|api| api.as_str().to_owned()),
        base_url: args.base_url.clone(),
        api_key_env: args.api_key_env.clone(),
        tokenizer: args.tokenizer.clone(),
        headers,
        reasoning: args.reasoning.then_some(true),
        responses_stateful: args.responses_stateful.then_some(true),
        chat_replay_reasoning_content: args.chat_replay_reasoning_content.then_some(true),
        chat_mistral_compat: args.chat_mistral_compat.then_some(true),
        max_tokens: args.max_tokens,
        temperature: args.temperature,
        stream_idle_timeout: args.stream_idle_timeout,
    };
    validate_daily_auth_ownership(config.as_ref(), &overrides, &|name: &str| ProcessConfigEnvironment.get(name))?;
    // Ordinary routes without stored accounts use environment credentials and
    // do not create an unrelated account database during catalog startup.
    if let Some(config) = broker_config {
        let identity = ara_cli::auth_broker_discover::client_identity(&discovery_host);
        let account = Arc::new(
            ara_cli::auth_broker_discover::discover_remote_auth_storage(
                config,
                ara_cli::auth_broker_discover::DiscoverRemoteOptions {
                    host: discovery_host,
                    client_options: Default::default(),
                    cache_path: None,
                    account_pool: None,
                    identity,
                },
                ara_cli::auth_storage::AuthStorageOptions {
                    config_key_resolver: key_resolver,
                    environment: Arc::new(ara_cli::auth_storage_registry::CatalogAuthEnvironment(
                        host.factory.environment.clone(),
                    )),
                    ..Default::default()
                },
                registry_cancel,
            )
            .await?,
        );
        *account_auth = Some(account.clone());
        host.credentials = Arc::new(ara_cli::auth_storage_registry::AuthStorageRegistryCredentials::new(
            account.as_ref().clone(),
            registry_cancel.clone(),
        ));
    } else if configured_provider.as_deref() == Some("openai-codex") || ara_home().join("agent/auth.db").exists() {
        let codex =
            open_codex_auth(reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build()?).await?;
        let account = Arc::new(ara_cli::auth_storage::AuthStorage::for_codex(
            codex,
            ara_cli::auth_storage::AuthStorageOptions {
                config_key_resolver: Arc::new(ara_cli::config_request_auth::ConfigStorageKeyResolver::new(
                    project_dir.to_path_buf(),
                    host.config_values.clone(),
                    host.config_environment.clone(),
                )),
                environment: Arc::new(ara_cli::auth_storage_registry::CatalogAuthEnvironment(
                    host.factory.environment.clone(),
                )),
                ..Default::default()
            },
        )?);
        #[cfg(feature = "test-fixture")]
        if let Ok(base) = std::env::var("ARA_TEST_CODEX_AUTH_BASE_URL") {
            let usage =
                ara_cli::codex_usage::CodexUsageProvider::with_fixture_endpoint_resolver(Arc::new(move |canonical| {
                    let path = reqwest::Url::parse(canonical).map(|url| url.path().to_owned()).unwrap_or_default();
                    format!("{}{path}", base.trim_end_matches('/'))
                }))?;
            account.register_usage_provider("openai-codex", Arc::new(usage))?;
        }
        // Install the owner before any Registry preflight can refresh a grant,
        // including failures before the request factory is built.
        *account_auth = Some(account.clone());
        host.credentials = Arc::new(ara_cli::auth_storage_registry::AuthStorageRegistryCredentials::new(
            account.as_ref().clone(),
            registry_cancel.clone(),
        ));
    }
    let registry = ara_cli::model_registry::ModelRegistry::open(
        ara_cli::model_config_file::ModelsConfigFile::new(&path)?,
        Default::default(),
        host,
    )
    .await?;
    if registry.config_error().is_some() {
        bail!("models configuration changed or could not be loaded; correct the file");
    }
    if let Some(provider) = configured_provider.as_ref() {
        let mut selected = ara_cli::model_patch::OrderedProviderSet::default();
        selected.insert(provider.as_str().into());
        registry
            .runtime()
            .refresh_discoverable_providers(selected, ara_cli::model_manager::ModelRefreshStrategy::OnlineIfUncached)
            .await?;
    }
    let config = registry.config();
    let selection = resolve_daily_selection_with_registry(
        config.as_ref(),
        &overrides,
        &|name: &str| ProcessConfigEnvironment.get(name),
        &registry,
    )?;
    args.api = Some(Api::from_str(selection.api.as_str(), false).map_err(anyhow::Error::msg)?);
    args.reasoning = selection.model.reasoning;
    args.max_tokens = selection.generation.max_tokens;
    args.temperature = selection.generation.temperature;
    validate_route_args(args)?;
    if args.api() == Api::OpenaiCodexResponses && args.mode == Mode::Rpc {
        bail!("OpenAI account mode currently supports print and REPL; use --mode text or json");
    }
    Ok(Route {
        model: selection.model.clone(),
        stream_options: StreamOptions::default(),
        daily: Some(selection),
        registry: Some(registry),
    })
}

/// Native message cuts retain a complete tool boundary and may split a turn
/// before its Assistant. Repeated summaries carry their prior summary and
/// cumulative raw provenance through the checked Session commit.
#[allow(clippy::too_many_arguments)]
async fn run_compaction(
    args: &Args,
    config: &AgentConfig,
    factory: &ProviderFactory,
    context: &mut Vec<Message>,
    sink: &HostSink,
    cancel: &CancellationToken,
    manual: bool,
) -> Result<bool> {
    run_compaction_with_selection(args, config, factory, context, sink, cancel, manual, None).await
}

#[allow(clippy::too_many_arguments)]
async fn run_compaction_with_selection(
    args: &Args,
    config: &AgentConfig,
    factory: &ProviderFactory,
    context: &mut Vec<Message>,
    sink: &HostSink,
    cancel: &CancellationToken,
    manual: bool,
    selection: Option<&ara_cli::snapcompact::ManualCompactArgs>,
) -> Result<bool> {
    let tokens = ara_cli::context_budget::tokenizer(&config.model)
        .count_messages(context, ara_agent::tokenizer::MessageCountOptions::default());
    if !manual && (args.compact_threshold == 0 || tokens <= args.compact_threshold) {
        return Ok(false);
    }
    let policy = rpc_host_settings::AutoCompactionPolicy::load(&ara_home().join("agent"))?;
    if !manual && !policy.enabled() {
        return Ok(false);
    }
    let settings = policy.recovery_settings()?;
    let methods = selection.and_then(|selection| selection.methods.as_ref()).unwrap_or(&settings.method_order);
    let focus = selection.and_then(|selection| selection.focus.as_deref());
    for method in methods {
        if cancel.is_cancelled() {
            return Err(ara_cli::handoff::cancelled());
        }
        let result = match method.as_str() {
            "remote" => {
                run_remote_compaction(args, config, factory, &settings, context, sink, cancel, manual, focus).await
            }
            "snapcompact" if focus.is_none_or(str::is_empty) => {
                run_snapcompact(
                    args,
                    config,
                    factory,
                    &settings,
                    context,
                    sink,
                    cancel,
                    selection.is_some_and(|selection| {
                        selection
                            .methods
                            .as_ref()
                            .is_some_and(|methods| methods.len() == 1 && methods[0] == "snapcompact")
                    }),
                )
                .await
            }
            "soft" => run_soft_compaction(args, config, factory, context, sink, cancel, manual, focus).await,
            // These automatic REPL methods retain their separately recorded
            // implementation scope; /handoff and /shake remain explicit commands.
            _ => continue,
        };
        match result {
            Ok(true) => return Ok(true),
            Ok(false) => {}
            Err(error)
                if ara_cli::remote_compaction::is_cancelled(&error) || sink.persist_failed.load(Ordering::SeqCst) =>
            {
                return Err(error);
            }
            Err(error)
                if selection
                    .is_some_and(|selection| selection.methods.as_ref().is_some_and(|methods| methods.len() == 1)) =>
            {
                return Err(error);
            }
            Err(error) => eprintln!("ara: {method} compaction failed ({error:#}); trying the next configured method"),
        }
    }
    Ok(false)
}

#[allow(clippy::too_many_arguments)]
async fn run_snapcompact(
    args: &Args,
    config: &AgentConfig,
    factory: &ProviderFactory,
    settings: &rpc_host_settings::RecoveryCompactionSettings,
    context: &mut Vec<Message>,
    sink: &HostSink,
    cancel: &CancellationToken,
    explicit: bool,
) -> Result<bool> {
    if !ara_cli::snapcompact::supports_images(factory.metadata.as_ref()) {
        if explicit {
            bail!("/compact snapcompact requires a vision-capable model")
        }
        return Ok(false);
    }
    if cancel.is_cancelled() {
        return Err(ara_cli::handoff::cancelled());
    }
    let snapshot = {
        let guard = sink.journal.lock().await;
        let Some(journal) = guard.as_ref().filter(|journal| journal.is_persistent()) else {
            if explicit {
                bail!("snapcompact requires a persistent Session")
            }
            return Ok(false);
        };
        journal.native_snapcompact_snapshot()?
    };
    let policy = ara_cli::snapcompact::SnapcompactPolicy {
        shape: settings.snapcompact_shape.clone(),
        reserve_tokens: settings.reserve_tokens,
        non_message_tokens: ara_cli::context_budget::non_message_tokens(
            &config.model,
            &config.system_prompt,
            &config.tools,
        ),
        pending_tokens: 0,
    };
    let source = snapshot.clone();
    let model = config.model.clone();
    let keep_tokens = args.compact_keep_tokens;
    let tokens_before =
        ara_cli::context_budget::tokenizer(&config.model).count_messages(context, Default::default()) as u64;
    let prepared = tokio::task::spawn_blocking(move || {
        ara_cli::snapcompact::prepare_snapcompact(&source, &model, keep_tokens, tokens_before, &policy)
    })
    .await?;
    if cancel.is_cancelled() {
        return Err(ara_cli::handoff::cancelled());
    }
    let Some(prepared) = prepared? else {
        if explicit {
            bail!("Nothing to compact (already compacted or session too small)")
        }
        return Ok(false);
    };
    let mut guard = sink.journal.lock().await;
    let journal = guard.as_mut().context("Session unavailable during snapcompact")?;
    if cancel.is_cancelled() {
        return Err(ara_cli::handoff::cancelled());
    }
    if let Err(error) = journal.commit_native_entry_snapcompact(
        &snapshot,
        &prepared.summary,
        &prepared.result.first_kept_entry_id,
        &prepared.window_source_entry_ids,
        tokens_before,
    ) {
        if error.history_published() {
            sink.persist_failed.store(true, Ordering::SeqCst);
            cancel.cancel();
            *context = journal.model_context();
        }
        return Err(error.into());
    }
    *context = ara_cli::remote_compaction::route_context(
        journal,
        &config.model,
        factory.metadata.as_ref(),
        &settings.remote,
        factory.route.remote_supports_images(),
    )?;
    eprintln!(
        "ara: snapcompact archive persisted in the current Session; estimated context {} tokens",
        prepared.tokens_after
    );
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
async fn run_remote_compaction(
    args: &Args,
    config: &AgentConfig,
    factory: &ProviderFactory,
    settings: &rpc_host_settings::RecoveryCompactionSettings,
    context: &mut Vec<Message>,
    sink: &HostSink,
    cancel: &CancellationToken,
    manual: bool,
    focus: Option<&str>,
) -> Result<bool> {
    use ara_agent::compaction::SummaryOptions;
    use ara_ai::remote_compaction::RemoteRequestOptions;
    use ara_cli::remote_compaction::{model_config, native_replay_available, prepare_remote};
    let remote_config = model_config(factory.metadata.as_ref())?;
    if !ara_agent::remote::remote_available(&config.model, &remote_config, &settings.remote) {
        return Ok(false);
    }
    let snapshot = {
        let guard = sink.journal.lock().await;
        let Some(journal) = guard.as_ref().filter(|journal| journal.is_persistent()) else { return Ok(false) };
        let available = native_replay_available(
            journal,
            &config.model,
            factory.metadata.as_ref(),
            &settings.remote,
            factory.route.remote_supports_images(),
        )?;
        journal.native_remote_snapshot(&config.model, available)?
    };
    let transport = factory.route.bind_remote(
        factory.client.clone(),
        RemoteRequestOptions {
            session_id: Some(snapshot.projection.session_id.clone()),
            generic_model: remote_config.model.clone(),
            ..Default::default()
        },
    );
    // Capture the same Session artifact owner before awaiting the side call.
    let artifacts = sink.artifacts.current_store();
    let prepared = prepare_remote(
        &snapshot,
        config,
        transport,
        &remote_config,
        &settings.remote,
        args.compact_keep_tokens,
        focus,
        settings.reserve_tokens,
        SummaryOptions {
            oneshot_retry: if manual { SummaryOptions::default().oneshot_retry } else { None },
            max_tokens: args.max_tokens,
        },
        Instant::now() + Duration::from_secs(120),
        cancel,
    )
    .await;
    let prepared = match prepared {
        Ok(Some(prepared)) => {
            record_remote_attempts(sink, &artifacts, &prepared.attempts).await?;
            prepared
        }
        Ok(None) => return Ok(false),
        Err(error) => {
            if let Some(failure) = error.downcast_ref::<ara_cli::remote_compaction::RemotePreparationError>() {
                record_remote_attempts(sink, &artifacts, &failure.attempts).await?;
            }
            return Err(error);
        }
    };
    let tokens_before = ara_cli::context_budget::tokenizer(&config.model)
        .count_messages(context, ara_agent::tokenizer::MessageCountOptions::default()) as u64;
    let mut guard = sink.journal.lock().await;
    let journal = guard.as_mut().context("Session unavailable")?;
    if cancel.is_cancelled() {
        return Err(ara_cli::handoff::cancelled());
    }
    if let Err(error) = prepared.commit(journal, &snapshot, tokens_before) {
        if error.history_published() {
            sink.persistence_failure(&error);
            let _ = factory.adopt_context(journal, context, sink);
        }
        return Err(error.into());
    }
    factory.adopt_context(journal, context, sink)?;
    eprintln!("ara: remote compaction saved in the current Session; {} request attempts", prepared.attempts.len());
    Ok(true)
}

async fn record_remote_attempts(
    sink: &HostSink,
    artifacts: &ara_cli::session_artifacts::SessionArtifacts,
    attempts: &[ara_ai::remote_compaction::RemoteAttempt],
) -> Result<()> {
    if attempts.is_empty() {
        return Ok(());
    }
    let receipts: Vec<serde_json::Value> = attempts
        .iter()
        .map(|attempt| {
            serde_json::json!({
                "elapsedMs":attempt.elapsed_ms, "usage":attempt.usage,
                "status":attempt.status, "failed":attempt.error.is_some(),
            })
        })
        .collect();
    let content = serde_json::to_string(&serde_json::json!({"method":"remote", "attempts":receipts}))?;
    match artifacts.save(&content, "remote-compaction").await {
        Ok(id) => {
            eprintln!("ara: remote compaction receipts: artifact://{id}");
            Ok(())
        }
        Err(error) => {
            sink.persistence_failure(&error);
            Err(error)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_soft_compaction(
    args: &Args,
    config: &AgentConfig,
    factory: &ProviderFactory,
    context: &mut Vec<Message>,
    sink: &HostSink,
    cancel: &CancellationToken,
    manual: bool,
    focus: Option<&str>,
) -> Result<bool> {
    use ara_agent::compaction::{
        SummaryOptions, select_native_entry_compaction_cut, summarize_native_entry_compaction_cut,
    };
    use ara_agent::tokenizer::MessageCountOptions;
    let model = &config.model;
    let provider = &config.provider;
    let tokens = ara_cli::context_budget::tokenizer(model).count_messages(context, MessageCountOptions::default());
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
    let Some(journal) = guard.as_mut().filter(|journal| journal.is_persistent()) else {
        notice("it needs a session (--no-session is set)".into());
        return Ok(false);
    };
    let snapshot = match journal.native_projected_compaction_snapshot() {
        Ok(s) => s,
        Err(e) => {
            notice(format!("the session source is unavailable ({e})"));
            return Ok(false);
        }
    };
    let sources = ara_cli::native_compaction::sources_for_model(&snapshot.entries, model);
    let previous_summary =
        snapshot.previous_summary.as_ref().map(|summary| summary.previous_summary_for_text_compaction());
    let previous_summary = previous_summary.as_deref();
    let cut = match select_native_entry_compaction_cut(&sources, args.compact_keep_tokens, previous_summary) {
        Ok(Some(cut)) => cut,
        Ok(None) => {
            notice("no earlier message prefix can be summarized at this retention target".into());
            return Ok(false);
        }
        Err(error) => {
            notice(format!("no earlier message prefix can be summarized ({error})"));
            return Ok(false);
        }
    };
    if cut.first_kept_index == 0 {
        notice("no earlier message prefix can be summarized at this retention target".into());
        return Ok(false);
    }
    let deadline = Instant::now() + Duration::from_secs(120);
    // The subscription endpoint rejects output caps. Omit the internal cap
    // from its wire call and check the observed summary before adoption below.
    let uncapped = CodexSummaryProvider(provider.as_ref());
    let summary_provider: &dyn ModelProvider =
        if model.api == "openai-codex-responses" { &uncapped } else { provider.as_ref() };
    let options = SummaryOptions {
        oneshot_retry: if manual { SummaryOptions::default().oneshot_retry } else { None },
        max_tokens: args.max_tokens,
    };
    let accepted = match summarize_native_entry_compaction_cut(
        &sources,
        &cut,
        previous_summary,
        focus,
        model,
        summary_provider,
        None,
        options,
        deadline,
        cancel,
    )
    .await
    {
        Ok(a) => a,
        Err(e) => {
            if e.kind == ara_agent::compaction::SummaryCallErrorKind::OutputBudgetExceeded {
                eprintln!("ara: Codex summary exceeds the local adoption budget; session untouched");
                return Ok(false);
            }
            // Show the stop reason, output tokens and the provider's status and
            // message so the cause is visible.
            let stop = e.stop_reason.map(|r| format!(", stop reason {}", r.as_str())).unwrap_or_default();
            let output =
                e.usage.as_ref().and_then(|u| u.output).map(|n| format!(", {n} output tokens")).unwrap_or_default();
            let status = e.provider_status.map(|s| format!(", HTTP {s}")).unwrap_or_default();
            let detail = e.provider_message.as_deref().map(|m| format!(": {}", sanitize_text(m))).unwrap_or_default();
            eprintln!("ara: summary call failed ({e}{stop}{output}{status}{detail}); session untouched");
            return Ok(false);
        }
    };
    if cancel.is_cancelled() {
        return Err(ara_cli::handoff::cancelled());
    }
    journal.commit_native_entry_compaction(
        &snapshot,
        &accepted.text,
        &cut.first_kept_entry_id,
        &accepted.window_source_entry_ids,
        tokens as u64,
    )?;
    factory.adopt_context(journal, context, sink)?;
    let kept = ara_cli::context_budget::tokenizer(model).count_messages(context, MessageCountOptions::default());
    eprintln!("ara: compacted {tokens} estimated tokens down to {kept}; summary persisted with source IDs");
    Ok(true)
}

struct CodexSummaryProvider<'a>(&'a dyn ModelProvider);

impl ModelProvider for CodexSummaryProvider<'_> {
    fn stream(
        &self,
        model: &Model,
        context: &ara_ai::Context,
        mut options: ara_ai::CallOptions,
    ) -> ara_ai::AssistantStream {
        options.max_tokens = None;
        self.0.stream(model, context, options)
    }
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

/// Reference-host consumer of the same account owner used by the Registry and
/// request route. Plain line output is not the native TUI/ACP usage renderer.
/// Amount formatting follows fixed OMP slash-commands/helpers/usage-report.ts
/// (MIT; Copyright 2025 Mario Zechner, 2025-2026 Can Bölük, 2026 Stencil Labs).
async fn run_repl_usage(
    factory: &ProviderFactory,
    session_id: Option<String>,
    refresh: bool,
    cancel: &CancellationToken,
) -> Result<()> {
    use ara_cli::auth_storage::{AuthRequestContext, FetchUsageReportsOptions};
    use ara_cli::auth_storage_policy::UsageUnit;
    let Some(storage) = factory.shared_account.as_ref().or(factory.account_auth.as_ref()) else {
        eprintln!("ara: provider quota reporting is unavailable for this route");
        return Ok(());
    };
    if refresh {
        storage.invalidate_usage_cache_and_notify(None, cancel).await?;
    }
    let selected = factory.route.model().clone();
    let registry = factory.registry.clone();
    let selected_provider = selected.provider.clone();
    let selected_url = selected.base_url.clone();
    let options = FetchUsageReportsOptions {
        context: AuthRequestContext { session_id, model_id: Some(selected.id.clone()), ..Default::default() },
        base_url_resolver: Some(Arc::new(move |provider| {
            if provider == selected_provider {
                Some(selected_url.clone())
            } else {
                registry.as_ref()?.get_provider_base_url(&provider.into()).ok()??.to_utf8().ok()
            }
        })),
        ..Default::default()
    };
    let Some(reports) = storage.fetch_usage_reports_with_options(&options, cancel).await? else {
        eprintln!("ara: provider quota reporting is unavailable");
        return Ok(());
    };
    if reports.is_empty() {
        eprintln!("ara: no provider quota reports available; remaining quota is unknown");
        return Ok(());
    }
    eprintln!("Usage");
    for report in reports {
        let account = report
            .metadata
            .as_ref()
            .and_then(|metadata| {
                ["email", "accountId", "projectId"].into_iter().find_map(|field| {
                    metadata.get(field)?.as_str().filter(|value| !value.is_empty()).map(str::to_owned)
                })
            })
            .unwrap_or_else(|| "account".into());
        eprintln!("  {} — {}", sanitize_text(&report.provider), sanitize_text(&account));
        if report.limits.is_empty() {
            eprintln!("    no limits reported");
        }
        for limit in report.limits {
            let amount = &limit.amount;
            let used = amount.used.or_else(|| amount.used_fraction.map(|fraction| fraction * 100.0));
            let unit = if amount.unit == UsageUnit::Percent {
                "%".to_owned()
            } else {
                format!(" {}", serde_json::to_value(amount.unit)?.as_str().unwrap_or("unknown"))
            };
            let used = used.map(|used| format!("{used:.2}{unit} used")).unwrap_or_else(|| "unknown used".into());
            let remaining = amount
                .remaining_fraction
                .or_else(|| amount.used_fraction.map(|used| (1.0 - used).max(0.0)))
                .map(|left| format!(" ({:.1}% left)", left * 100.0))
                .unwrap_or_default();
            eprintln!("    {}: {used}{remaining}", sanitize_text(&limit.label));
        }
    }
    Ok(())
}

/// Session ownership and route settings for REPL conversation boundaries.
struct ReplSession<'a> {
    /// `None` under `--no-session`.
    dir: Option<PathBuf>,
    cwd: &'a Path,
    model_ref: &'a str,
    skills: &'a [ara_discovery::LoadedSkill],
    provider_factory: &'a ProviderFactory,
    mcp_config: &'a Option<McpServerConfig>,
    artifact_router: Arc<ara_cli::session_artifacts::ArtifactUriRouter>,
}

#[allow(clippy::too_many_arguments)]
async fn run_local_shake(
    mode: &str,
    model: &ara_ai::Model,
    factory: &ProviderFactory,
    context: &mut Vec<Message>,
    sink: &HostSink,
    artifacts: &ara_cli::session_artifacts::SessionArtifacts,
    non_message: usize,
    cancel: &CancellationToken,
) -> Result<bool> {
    use ara_agent::compaction::local_reduction::ShakeConfig;
    use ara_cli::local_reduction::{
        HostReductionPolicy, prepare_images_after_boundary, prepare_shake, prepare_thinking_after_boundary,
    };
    let settings = rpc_host_settings::AutoCompactionPolicy::load(&ara_home().join("agent"))?.recovery_settings()?;
    let (snapshot, native_boundary) = {
        let guard = sink.journal.lock().await;
        let journal = guard.as_ref().context("Session unavailable")?;
        (
            journal.raw_reduction_snapshot()?,
            ara_cli::remote_compaction::native_reduction_boundary(
                journal,
                model,
                factory.metadata.as_ref(),
                &settings.remote,
                factory.route.remote_supports_images(),
            )?,
        )
    };
    let policy = HostReductionPolicy {
        keep_boundary_id: native_boundary.clone().or_else(|| {
            snapshot
                .entries
                .iter()
                .rev()
                .find(|entry| entry.kind == "compaction")
                .and_then(|entry| entry.raw["firstKeptEntryId"].as_str())
                .map(str::to_owned)
        }),
        ..Default::default()
    };
    let mut prepared = match mode {
        "elide" => {
            prepare_shake(snapshot, model, &ShakeConfig::aggressive(), &policy, artifacts, ara_ai::now_ms()).await?
        }
        "images" => prepare_images_after_boundary(snapshot, model, native_boundary.as_deref())?,
        "thinking" => prepare_thinking_after_boundary(snapshot, model, native_boundary.as_deref())?,
        _ => bail!("Unknown shake mode: {mode}. Expected elide, images or thinking"),
    };
    if cancel.is_cancelled() {
        bail!("Shake aborted");
    }
    if mode == "elide"
        && let Some(edit) =
            ara_cli::context_budget::anchor_edit(&prepared.snapshot, &prepared.entry_tokens_freed, non_message)
    {
        prepared.edits.push(edit);
    }
    if prepared.edits.is_empty() {
        eprintln!("ara: shake {mode}: nothing to reduce");
        return Ok(false);
    }
    let mut guard = sink.journal.lock().await;
    let journal = guard.as_mut().context("Session unavailable")?;
    if let Err(error) = journal.commit_reduction(&prepared.snapshot, &prepared.edits) {
        if error.history_published() {
            sink.persistence_failure(&error);
            let _ = factory.adopt_context(journal, context, sink);
        }
        return Err(error.into());
    }
    factory.adopt_context(journal, context, sink)?;
    let counts = &prepared.plan.counts;
    eprintln!(
        "ara: shake {mode}: {} tool results, {} blocks, {} images, {} thinking blocks; ~{} tokens freed{}",
        counts.tool_results_dropped,
        counts.blocks_dropped,
        counts.images_dropped,
        counts.thinking_blocks_dropped,
        prepared.tokens_freed,
        prepared.artifact_id.map(|id| format!("; recover: artifact://{id}")).unwrap_or_default()
    );
    Ok(true)
}

async fn run_repl_handoff(
    config: &AgentConfig,
    factory: &ProviderFactory,
    keep_tokens: usize,
    focus: Option<&str>,
    context: &mut Vec<Message>,
    sink: &HostSink,
    cancel: &CancellationToken,
) -> Result<bool> {
    let snapshot = sink.journal.lock().await.as_ref().context("Session unavailable")?.native_handoff_snapshot()?;
    let side = factory.route.bind_side_request(factory.client.clone(), &snapshot.projection.session_id);
    let deadline = config.deadline.unwrap_or_else(|| Instant::now() + Duration::from_secs(120));
    let Some(prepared) = ara_cli::handoff::prepare_handoff(
        &snapshot,
        context,
        config,
        side.as_ref(),
        keep_tokens,
        focus,
        false,
        deadline,
        cancel,
    )
    .await?
    else {
        return Ok(false);
    };
    if cancel.is_cancelled() {
        bail!("Handoff cancelled");
    }
    let tokens_before = ara_cli::context_budget::tokenizer(&config.model)
        .count_messages(context, ara_agent::tokenizer::MessageCountOptions::default()) as u64;
    let mut guard = sink.journal.lock().await;
    let journal = guard.as_mut().context("Session unavailable")?;
    if cancel.is_cancelled() {
        bail!("Handoff cancelled");
    }
    if let Err(error) = journal.commit_native_entry_handoff(
        &snapshot,
        &prepared.summary,
        &prepared.first_kept_entry_id,
        &prepared.window_source_entry_ids,
        tokens_before,
    ) {
        if error.history_published() {
            sink.persistence_failure(&error);
            let _ = factory.adopt_context(journal, context, sink);
        }
        return Err(error.into());
    }
    factory.adopt_context(journal, context, sink)?;
    eprintln!("ara: handoff saved in the current Session; recent history kept");
    Ok(true)
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
    let mut provider = provider.clone();
    let mut system_prompt = system_prompt.to_vec();
    let mut hooks = hooks.clone();
    eprintln!("ara: interactive session ({}). /help for commands.", model.id);
    let current_path =
        || async { sink.journal.lock().await.as_ref().filter(|j| j.is_persistent()).map(|j| j.path().to_path_buf()) };
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
                eprintln!(
                    "commands: /help, /new, /clear, /compact, /exit, /usage [refresh], /shake [elide|images|thinking], /handoff [focus], /skill:<name> [arguments]"
                );
                for skill in session.skills {
                    eprintln!("  /skill:{} — {}", sanitize_text(&skill.name), sanitize_text(&skill.description));
                }
                eprintln!("Ctrl+C during a turn cancels it; Ctrl+C at the prompt exits.");
                continue;
            }
            "/usage" | "/usage refresh" => {
                let token = cancel.child_token();
                let id = sink.journal.lock().await.as_ref().map(|journal| journal.session_id().to_owned());
                let step = run_repl_usage(session.provider_factory, id, input == "/usage refresh", &token);
                if let Err(error) = interruptible(step, &token, &mut interrupts).await {
                    eprintln!("ara: usage query failed ({error:#})");
                }
                continue;
            }
            command if command == "/compact" || command.starts_with("/compact ") => {
                let selection = match ara_cli::snapcompact::parse_manual_args(command.trim_start_matches("/compact")) {
                    Ok(selection) => selection,
                    Err(error) => {
                        eprintln!("ara: {error}");
                        continue;
                    }
                };
                let token = cancel.child_token();
                let config = AgentConfig {
                    model: model.clone(),
                    provider: provider.clone(),
                    system_prompt: system_prompt.clone(),
                    tools: tools.to_vec(),
                    tool_choice: None,
                    max_tokens: args.max_tokens,
                    temperature: args.temperature,
                    deadline: None,
                    max_model_calls: args.max_model_calls,
                    hooks: hooks.clone(),
                };
                let step = run_compaction_with_selection(
                    args,
                    &config,
                    session.provider_factory,
                    context,
                    sink,
                    &token,
                    true,
                    Some(&selection),
                );
                match interruptible(step, &token, &mut interrupts).await {
                    Ok(true) => {
                        let id =
                            sink.journal.lock().await.as_ref().context("Session unavailable")?.session_id().to_owned();
                        provider = session
                            .provider_factory
                            .route
                            .bind_codex_session(session.provider_factory.client.clone(), id);
                    }
                    Ok(false) => {}
                    Err(e) => eprintln!("ara: compaction failed ({e:#}); inspect the Session receipt"),
                }
                if sink.persist_failed.load(Ordering::SeqCst) {
                    break 1;
                }
                continue;
            }
            "/clear" => {
                // REPL commands run only between turns. Prepare the next base
                // prompt with retained tools, then publish the reset only after
                // its journal boundary is accepted. MCP stays connected.
                let setup = match prepare_cli_setup_with_retained_skills(
                    args,
                    session.cwd,
                    model,
                    session.mcp_config,
                    Some(tools.to_vec()),
                    ToolOverlay::default(),
                    Some(session.skills.to_vec()),
                )
                .await
                {
                    Ok(setup) => setup,
                    Err(error) => {
                        eprintln!("ara: could not clear context ({error:#}); session kept");
                        continue;
                    }
                };
                let factory = session.provider_factory;
                let fresh_provider =
                    factory.route.bind_codex_session(factory.client.clone(), uuid::Uuid::now_v7().to_string());
                let mut guard = sink.journal.lock().await;
                if let Some(journal) = guard.as_mut()
                    && let Err(error) = journal.append_reset_boundary()
                {
                    eprintln!("ara: could not clear context ({error:#}); session kept");
                    continue;
                }
                context.clear();
                provider = fresh_provider;
                system_prompt = setup.system_prompt;
                hooks = setup.hooks;
                turn = 0;
                eprintln!("ara: context cleared; original Session history kept");
                continue;
            }
            "/new" => {
                let mut guard = sink.journal.lock().await;
                match (guard.as_mut(), &session.dir) {
                    (Some(journal), Some(dir)) => {
                        let fresh =
                            SessionJournal::create_with_blob_directory(dir, session.cwd, &ara_blobs_directory())
                                .and_then(|mut j| {
                                    j.append_model_change(session.model_ref)?;
                                    Ok(j)
                                });
                        match fresh {
                            Ok(fresh) => {
                                let previous = std::mem::replace(journal, fresh);
                                session
                                    .artifact_router
                                    .bind(ara_cli::session_artifacts::SessionArtifacts::for_journal(journal));
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
                    (Some(journal), None) => {
                        *journal = SessionJournal::in_memory(ephemeral_header(session.cwd, None))?;
                        session
                            .artifact_router
                            .bind(ara_cli::session_artifacts::SessionArtifacts::for_journal(journal));
                        eprintln!("ara: new conversation (not persisted)");
                    }
                    _ => eprintln!("ara: new conversation (not persisted)"),
                }
                let factory = session.provider_factory;
                if model.api == "openai-codex-responses" {
                    let header = guard
                        .as_ref()
                        .map(|j| j.header().clone())
                        .unwrap_or_else(|| ephemeral_header(session.cwd, None));
                    let id = header["id"].as_str().context("new Session has no ID")?;
                    provider = factory.route.bind_codex_session(factory.client.clone(), id.to_owned());
                }
                context.clear();
                turn = 0;
                continue;
            }
            _ => {}
        }
        if input == "/shake" || input.starts_with("/shake ") {
            let mode = input.strip_prefix("/shake").unwrap().trim();
            let mode = if mode.is_empty() { "elide" } else { mode };
            let store = session.artifact_router.current_store();
            let token = cancel.child_token();
            let non_message = ara_cli::context_budget::non_message_tokens(model, &system_prompt, tools);
            let step =
                run_local_shake(mode, model, session.provider_factory, context, sink, &store, non_message, &token);
            match interruptible(step, &token, &mut interrupts).await {
                Ok(true) => provider = session.provider_factory.build(),
                Ok(false) => {}
                Err(error) => eprintln!("ara: shake failed ({error:#}); inspect the Session receipt"),
            }
            if sink.persist_failed.load(Ordering::SeqCst) {
                break 1;
            }
            continue;
        }
        if input == "/handoff" || input.starts_with("/handoff ") {
            let focus = input.strip_prefix("/handoff").unwrap().trim();
            let token = cancel.child_token();
            let config = AgentConfig {
                model: model.clone(),
                provider: provider.clone(),
                system_prompt: system_prompt.clone(),
                tools: tools.to_vec(),
                tool_choice: None,
                max_tokens: args.max_tokens,
                temperature: args.temperature,
                deadline: args.max_time.map(|seconds| Instant::now() + Duration::from_secs_f64(seconds.max(0.0))),
                max_model_calls: args.max_model_calls,
                hooks: hooks.clone(),
            };
            let step = run_repl_handoff(
                &config,
                session.provider_factory,
                args.compact_keep_tokens,
                (!focus.is_empty()).then_some(focus),
                context,
                sink,
                &token,
            );
            match interruptible(step, &token, &mut interrupts).await {
                Ok(true) => {
                    let id = sink.journal.lock().await.as_ref().context("Session unavailable")?.session_id().to_owned();
                    provider =
                        session.provider_factory.route.bind_codex_session(session.provider_factory.client.clone(), id);
                }
                Ok(false) => {}
                Err(error) => eprintln!("ara: handoff failed ({error:#}); inspect the Session receipt"),
            }
            if sink.persist_failed.load(Ordering::SeqCst) {
                break 1;
            }
            continue;
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
        // Match OMP input-controller's submitted text.trim() before display
        // sanitization, which would delete internal JS whitespace boundaries.
        let draft = ara_prompt::js::trim(&raw);
        let prep = skill_command::prepare(draft, &raw, session.skills, &token, config.deadline);
        let skill = match interruptible(prep, &token, &mut interrupts).await {
            Ok(skill) => skill,
            Err(skill_command::PreparationError::Load(error)) => {
                eprintln!("ara: failed to load skill ({}); session kept", sanitize_text(&error));
                continue;
            }
            Err(skill_command::PreparationError::Cancelled) => {
                eprintln!("ara: turn {turn} cancelled while loading skill; session kept");
                continue;
            }
            Err(skill_command::PreparationError::Deadline) => {
                eprintln!("ara: turn {turn} hit the deadline while loading skill; session kept");
                continue;
            }
        };
        let projected =
            skill.as_ref().map_or_else(|| Message::User(UserMessage::text(input)), |skill| skill.model_message());
        let skill_sink = skill.as_ref().map(|prompt| skill_command::SkillPromptSink {
            host: sink,
            prompt,
            projected: &projected,
            recorded: AtomicBool::new(false),
        });
        let turn_sink: &dyn AgentEventSink = skill_sink.as_ref().map_or(sink, |sink| sink as &dyn AgentEventSink);
        let step = agent_loop(vec![projected.clone()], context, &config, &token, turn_sink);
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
            let step = run_compaction(args, &config, session.provider_factory, context, sink, &token, false);
            match interruptible(step, &token, &mut interrupts).await {
                Ok(true) => {
                    let id = sink.journal.lock().await.as_ref().context("Session unavailable")?.session_id().to_owned();
                    provider =
                        session.provider_factory.route.bind_codex_session(session.provider_factory.client.clone(), id);
                }
                Ok(false) => {}
                Err(e) => eprintln!("ara: auto-compaction failed ({e:#}); inspect the Session receipt"),
            }
            if sink.persist_failed.load(Ordering::SeqCst) {
                break 1;
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

fn recover_session(j: &mut SessionJournal) -> Result<()> {
    if !j.report.blob_warnings.is_empty() {
        eprintln!(
            "ara: {} missing or malformed image blob reference(s); restore the Session blob store to recover those images",
            j.report.blob_warnings.len()
        );
    }
    if j.report.malformed_records > 0 {
        eprintln!("ara: skipped {} malformed record(s) in {}", j.report.malformed_records, j.path().display());
    }
    let undecodable = j.undecodable_messages();
    if undecodable > 0 {
        eprintln!("ara: {undecodable} message(s) in the session could not be decoded and are not sent to the model");
    }
    let recovery = j.recover_interrupted_tool_calls()?;
    if !recovery.paired.is_empty() {
        eprintln!(
            "ara: tool call(s) {} were interrupted before a result was recorded; their effects are unknown and they were not re-run",
            recovery.paired.join(", ")
        );
    }
    if !recovery.unpaired_earlier.is_empty() {
        eprintln!("ara: earlier tool call(s) {} have no recorded result", recovery.unpaired_earlier.join(", "));
    }
    Ok(())
}

struct CliSetup {
    system_prompt: Vec<String>,
    tools: Vec<Arc<dyn ara_agent::AgentTool>>,
    hooks: Arc<dyn LoopHooks>,
    skills: Vec<ara_discovery::LoadedSkill>,
    tool_context: Option<ToolContext>,
}

#[derive(Default)]
struct ToolOverlay {
    tools: Vec<Arc<dyn ara_agent::AgentTool>>,
    labels: std::collections::HashMap<String, String>,
}

async fn prepare_cli_setup(
    args: &Args,
    cwd: &Path,
    model: &Model,
    mcp_config: &Option<McpServerConfig>,
    retained_tools: Option<Vec<Arc<dyn ara_agent::AgentTool>>>,
    overlay: ToolOverlay,
) -> Result<CliSetup> {
    prepare_cli_setup_with_retained_skills(args, cwd, model, mcp_config, retained_tools, overlay, None).await
}

async fn prepare_cli_setup_with_retained_skills(
    args: &Args,
    cwd: &Path,
    model: &Model,
    mcp_config: &Option<McpServerConfig>,
    retained_tools: Option<Vec<Arc<dyn ara_agent::AgentTool>>>,
    overlay: ToolOverlay,
    retained_skills: Option<Vec<ara_discovery::LoadedSkill>>,
) -> Result<CliSetup> {
    // Context files, skills, SYSTEM.md and APPEND_SYSTEM.md from the host's
    // locations: native `$ARA_HOME/agent` and `.ara/`, foreign tools per
    // upstream defaults.
    let home = user_home();
    let mut dirs = HostDirs::ara(&home).with_env(|k| std::env::var(k).ok());
    dirs.native_user_dir = ara_home().join("agent");
    let discovery = Discovery::new(&home, dirs, ProviderPolicy::default());
    let skills_settings = SkillsSettings {
        enabled: !args.no_skills,
        source_switches: Some(args.skill_sources),
        include_skills: args
            .skills
            .as_deref()
            .map(|s| s.split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect())
            .unwrap_or_default(),
        ..SkillsSettings::default()
    };
    // Context reset refreshes rules and base prompt files, while slash command
    // dispatch, advertised skills and retained tools keep one Skill snapshot.
    let (skills, skill_warnings) = retained_skills
        .map(|skills| (skills, Vec::new()))
        .unwrap_or_else(|| discovery.load_skills(cwd, &skills_settings));
    for warning in &skill_warnings {
        let at = if warning.skill_path.is_empty() { String::new() } else { format!(" ({})", warning.skill_path) };
        eprintln!("ara: skill warning{at}: {}", warning.message);
    }

    let enabled: Vec<&str> = args.tools.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
    let mut tool_context = None;
    let mut tools = if let Some(tools) = retained_tools {
        tools
    } else {
        let mut tool_ctx =
            ToolContext::new(cwd.to_path_buf()).with_edit(args.edit_mode, enabled.contains(&"edit")).with_skills(
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
        tool_context = Some(tool_ctx.clone());
        let ast_ctx = tool_ctx.clone();
        let mut tools: Vec<_> =
            builtin_tools(tool_ctx).into_iter().filter(|t| enabled.contains(&t.definition().name.as_str())).collect();
        if enabled.contains(&"ast_grep") {
            tools.push(Arc::new(ara_tools::ast_grep::AstGrepTool { ctx: ast_ctx }));
        }
        if let Some(config) = mcp_config.clone() {
            let mcp_tools =
                ara_mcp::connect(config, cwd, &args.mcp_allow, &TOOL_NAMES).await.map_err(anyhow::Error::msg)?;
            tools.extend(mcp_tools);
        }

        tools
    };
    for tool in &overlay.tools {
        let name = &tool.definition().name;
        if tools.iter().any(|existing| existing.definition().name == *name) {
            anyhow::bail!("RPC host tool \"{name}\" conflicts with an existing tool");
        }
    }
    tools.extend(overlay.tools);

    let prompt_tools: Vec<PromptTool> = tools
        .iter()
        .map(|t| {
            let name = t.definition().name.clone();
            let label = overlay
                .labels
                .get(&name)
                .cloned()
                .unwrap_or_else(|| name.get(..1).map(|f| f.to_uppercase() + &name[1..]).unwrap_or_default());
            PromptTool { name, label }
        })
        .collect();
    // Flags win; otherwise the discovered files (upstream `main.ts`
    // `discoverSystemPromptFile` / `discoverAppendSystemPromptFile`).
    let mut prompt_warnings = Vec::new();
    let discovered = |name: &str| discovery.discover_prompt_file(cwd, name).map(|p| p.to_string_lossy().into_owned());
    let system_source = args.system_prompt.clone().or_else(|| discovered("SYSTEM.md"));
    let append_source = args.append_system_prompt.clone().or_else(|| discovered("APPEND_SYSTEM.md"));
    let custom_prompt = resolve_prompt_input(system_source.as_deref(), "system prompt", &mut prompt_warnings);
    let append_prompt = resolve_prompt_input(append_source.as_deref(), "append system prompt", &mut prompt_warnings);
    let options = SystemPromptOptions {
        custom_prompt,
        append_prompt,
        tools: Some(prompt_tools),
        skills: Some(skills),
        model: Some(model.id.clone()),
        urls: InternalUrls { skill: enabled.contains(&"read"), ..InternalUrls::default() },
        ..SystemPromptOptions::default()
    };
    let built = build_system_prompt(&discovery, cwd, &options).context("building the system prompt")?;
    for warning in prompt_warnings.iter().chain(&built.warnings) {
        eprintln!("ara: warning: {warning}");
    }
    let system_prompt = built.blocks;
    let hooks: Arc<dyn LoopHooks> =
        Arc::new(CliHooks { reminder: DateCwdReminder::new(), cwd: cwd.to_string_lossy().replace('\\', "/") });

    Ok(CliSetup { system_prompt, tools, hooks, skills: options.skills.unwrap_or_default(), tool_context })
}

// Clone the transport settings, but allocate native provider state per logical
// Session. An unchanged reload deliberately keeps its existing provider.
struct ProviderFactory {
    client: reqwest::Client,
    route: ara_cli::model_route::PreparedRoute,
    account_auth: Option<Arc<ara_cli::auth_storage::AuthStorage>>,
    shared_account: Option<Arc<ara_cli::auth_storage::AuthStorage>>,
    registry: Option<ara_cli::model_registry::ModelRegistry>,
    metadata: Option<serde_json::Value>,
}

impl ProviderFactory {
    fn context_for(&self, journal: &SessionJournal) -> Result<Vec<Message>> {
        let settings = rpc_host_settings::AutoCompactionPolicy::load(&ara_home().join("agent"))?.recovery_settings()?;
        ara_cli::remote_compaction::route_context(
            journal,
            self.route.model(),
            self.metadata.as_ref(),
            &settings.remote,
            self.route.remote_supports_images(),
        )
    }

    fn adopt_context(&self, journal: &SessionJournal, context: &mut Vec<Message>, sink: &HostSink) -> Result<()> {
        match self.context_for(journal) {
            Ok(messages) => {
                *context = messages;
                Ok(())
            }
            Err(error) => {
                sink.persistence_failure(&error);
                Err(error)
            }
        }
    }

    async fn daily(
        client: reqwest::Client,
        mut selection: ara_cli::daily_model_config::DailySelection,
        session_id: Option<String>,
        cwd: &Path,
        cancel: &CancellationToken,
        shared_account: Option<Arc<ara_cli::auth_storage::AuthStorage>>,
        registry: Option<ara_cli::model_registry::ModelRegistry>,
    ) -> Result<Self> {
        use ara_cli::daily_model_config::DailyAuthSource;
        use ara_cli::model_route::{
            FixedRequestAuth, HostDefaultRequestAuth, PreparedRoute, ProtocolOptions, RequestAuthResolver,
        };
        // Native SessionStats consumes headers independently of the branch
        // that supplies credentials or participates in authentication retry.
        let usage_headers = shared_account.as_ref().map(|owner| {
            let observer = ara_cli::session_usage_headers::SessionUsageHeaders::new(
                owner.as_ref().clone(),
                session_id.clone(),
                None,
            );
            if let Some(registry) = registry.clone() { observer.with_registry(registry) } else { observer }
        });
        let usage_account = shared_account.clone();
        if let ProtocolOptions::CodexResponses(options) = &mut selection.protocol {
            options.session_id = session_id.clone();
        }
        let (auth, account_auth): (Arc<dyn RequestAuthResolver>, _) = match selection.auth_source {
            DailyAuthSource::Fixed(lease) => {
                if selection.host_default_auth
                    && let Some(account) = shared_account.clone()
                {
                    let auth = HostDefaultRequestAuth::new(
                        account
                            .session_resolver(session_id.clone(), Some(selection.model.base_url.clone()))
                            .with_environment_lease(&lease),
                        lease,
                    );
                    (Arc::new(auth), Some(account))
                } else {
                    (Arc::new(FixedRequestAuth::new(lease)), None)
                }
            }
            DailyAuthSource::OpenAiCodex => {
                let account = shared_account.clone().context("OpenAI account owner is unavailable")?;
                (account.session_resolver(session_id.clone(), Some(selection.model.base_url.clone())), Some(account))
            }
            DailyAuthSource::Configured(spec) => {
                let account = if spec.codex_account {
                    Some(shared_account.context("OpenAI account owner is unavailable")?)
                } else if selection.host_default_auth {
                    shared_account
                } else {
                    None
                };
                let inner = account.as_ref().map(|account| {
                    let primary: Arc<dyn RequestAuthResolver> = account
                        .session_resolver(session_id.clone(), Some(selection.model.base_url.clone()))
                        .with_environment_lease(&spec.base);
                    if spec.codex_account {
                        primary
                    } else {
                        Arc::new(HostDefaultRequestAuth::new(primary, spec.base.clone()))
                    }
                });
                let auth = ara_cli::config_request_auth::ConfigRequestAuth::new(spec, cwd.to_path_buf(), inner);
                auth.prepare(cancel)
                    .await
                    .map_err(|_| anyhow::anyhow!("configured authentication command preparation failed"))?;
                (Arc::new(auth), account)
            }
        };
        let loop_guard_policy = configured_loop_guard_policy(selection.loop_guard_policy)?;
        let mut route = PreparedRoute::new(selection.model, selection.protocol, auth, 0)
            .map_err(|error| anyhow::anyhow!("preparing configured model route: {error:?}"))?
            .with_loop_guard_policy(loop_guard_policy);
        if let Some(owner) = usage_headers {
            route = route.with_usage_headers(owner);
        }
        Ok(Self {
            client,
            route,
            account_auth,
            shared_account: usage_account,
            registry,
            metadata: Some(selection.metadata),
        })
    }

    fn startup(
        client: reqwest::Client,
        model: Model,
        api: Api,
        mut options: StreamOptions,
        anthropic_strict_tools: bool,
        responses_stateful: bool,
    ) -> Result<Self> {
        use ara_cli::model_route::{
            CredentialIdentity, FixedRequestAuth, PreparedRoute, ProtocolOptions, RequestAuthLease,
            is_credential_header,
        };
        let (auth_headers, ordinary_headers) =
            options.extra_headers.into_iter().partition(|(name, _)| is_credential_header(name));
        options.extra_headers = ordinary_headers;
        let auth = Arc::new(FixedRequestAuth::new(
            RequestAuthLease::new(CredentialIdentity::Runtime, options.api_key.take()).with_headers(auth_headers),
        ));
        let protocol = match api {
            Api::AnthropicMessages => ProtocolOptions::Anthropic(ara_ai::providers::anthropic::StreamOptions {
                strict_tools: anthropic_strict_tools.then_some(true),
                extra_headers: options.extra_headers,
                first_event_timeout: options.first_event_timeout,
                idle_timeout: options.idle_timeout,
                retry: options.retry,
                ..Default::default()
            }),
            Api::OpenaiCompletions => ProtocolOptions::Completions(options),
            Api::OpenaiResponses => ProtocolOptions::Responses(ResponsesStreamOptions {
                extra_headers: options.extra_headers,
                first_event_timeout: options.first_event_timeout,
                idle_timeout: options.idle_timeout,
                retry: options.retry,
                stateful_responses: responses_stateful,
                ..Default::default()
            }),
            Api::OpenaiCodexResponses => bail!("OpenAI account routes require the daily account resolver"),
            Api::ProxyAuto => bail!("proxy discovery must resolve to a concrete protocol"),
        };
        let route = PreparedRoute::new(model, protocol, auth, 0)
            .map_err(|error| anyhow::anyhow!("preparing startup model route: {error:?}"))?;
        let loop_guard_policy = configured_loop_guard_policy(route.loop_guard_policy())?;
        let route = route.with_loop_guard_policy(loop_guard_policy);
        Ok(Self { client, route, account_auth: None, shared_account: None, registry: None, metadata: None })
    }

    fn build(&self) -> Arc<dyn ModelProvider> {
        self.route.bind(self.client.clone(), None)
    }
}

fn configured_loop_guard_policy(
    mut policy: ara_ai::thinking_loop::LoopGuardPolicy,
) -> Result<ara_ai::thinking_loop::LoopGuardPolicy> {
    let settings = rpc_host_settings::LoopGuardSettings::load(&ara_home().join("agent"))?;
    policy.enabled = settings.enabled;
    policy.check_assistant_content = settings.check_assistant_content;
    Ok(policy)
}

async fn open_codex_auth(client: reqwest::Client) -> Result<Arc<ara_cli::openai_codex_auth::OpenAiCodexAuth>> {
    #[cfg(feature = "test-fixture")]
    if let Ok(base) = std::env::var("ARA_TEST_CODEX_AUTH_BASE_URL") {
        return Ok(Arc::new(
            ara_cli::openai_codex_auth::OpenAiCodexAuth::open_with_endpoints(
                ara_home().join("agent/auth.db"),
                client,
                &base,
            )
            .await?,
        ));
    }
    Ok(Arc::new(ara_cli::openai_codex_auth::OpenAiCodexAuth::open(ara_home().join("agent/auth.db"), client).await?))
}

async fn run_auth_command(command: AuthCommand) -> Result<i32> {
    let provider = match &command {
        AuthCommand::Login { provider } | AuthCommand::Logout { provider } => provider,
    };
    if provider != "openai-codex" {
        bail!("account login currently supports openai-codex; other providers are deferred");
    }
    let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build()?;
    let auth = open_codex_auth(client).await?;
    match command {
        AuthCommand::Logout { .. } => {
            auth.logout().await?;
            eprintln!("ara: signed out of OpenAI");
        }
        AuthCommand::Login { .. } => {
            let cancel = CancellationToken::new();
            let mut interrupts = listen_for_interrupts()?;
            let stop = cancel.clone();
            let listener = tokio::spawn(async move {
                if interrupts.recv().await.is_some() {
                    stop.cancel();
                }
            });
            let result = auth
                .login_device(&cancel, |info| {
                    eprintln!("ara: open {} and enter code {}", info.verification_url, info.user_code);
                })
                .await;
            listener.abort();
            result?;
            eprintln!("ara: signed in to OpenAI; use --provider openai-codex --model <model-id>");
        }
    }
    Ok(0)
}

fn ephemeral_header(cwd: &Path, parent: Option<&str>) -> serde_json::Value {
    let mut header = serde_json::json!({
        "type":"session", "version":ara_session::CURRENT_SESSION_VERSION,
        "id":uuid::Uuid::now_v7().to_string(),
        "timestamp":chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
        "cwd":cwd.to_string_lossy()
    });
    if let Some(parent) = parent.filter(|parent| !parent.is_empty()) {
        header["parentSession"] = serde_json::json!(parent);
    }
    header
}

async fn run(args: Args) -> Result<i32> {
    let mut account_auth = None;
    let registry_cancel = CancellationToken::new();
    let result = run_inner(args, &mut account_auth, &registry_cancel).await;
    registry_cancel.cancel();
    // Normal error/deadline/first-interrupt exits must finish any dispatched
    // refresh settlement before main shuts down the runtime. Hard process
    // termination still cannot prove a remote grant's outcome.
    if let Some(account) = account_auth {
        account.close_and_wait().await;
    }
    ara_cli::config_request_auth::wait_for_config_settlement().await;
    ara_cli::auth_broker_snapshot_cache::wait_for_cache_writes().await;
    result
}

async fn run_inner(
    mut args: Args,
    account_auth: &mut Option<Arc<ara_cli::auth_storage::AuthStorage>>,
    registry_cancel: &CancellationToken,
) -> Result<i32> {
    if let Some(command) = args.command.take() {
        return run_auth_command(command).await;
    }
    let _ = args.print;
    let rpc_mode = args.mode == Mode::Rpc;
    if rpc_mode && (args.repl || !args.prompts.is_empty() || args.print) {
        bail!("--mode rpc reads commands from stdin; omit --repl, --print and positional prompts");
    }
    // Proxy auto-discovery explicitly forbids resume. Reject that pure CLI
    // contract before looking for a journal; config-selected APIs are resolved
    // and validated by the route projection below.
    if args.api() == Api::ProxyAuto {
        validate_route_args(&args)?;
    }
    let explicit_cwd = match &args.cwd {
        Some(c) => {
            let canonical = std::fs::canonicalize(c).with_context(|| format!("--cwd {}", c.display()))?;
            Some(plain_drive_path(canonical))
        }
        None => None,
    };
    let launch_cwd = explicit_cwd.clone().map(Ok).unwrap_or_else(std::env::current_dir)?;
    let mut cwd = launch_cwd.clone();
    let session_dir =
        (!args.no_session).then(|| args.session_dir.clone().unwrap_or_else(|| default_session_dir(&launch_cwd)));
    let resume_path = match (&session_dir, &args.resume, args.continue_session) {
        (Some(_), Some(path), _) => Some(path.clone()),
        (Some(dir), None, true) => {
            Some(latest_session(dir).with_context(|| format!("no session to continue in {}", dir.display()))?)
        }
        _ => None,
    };
    // Read the resumed journal once to establish helper cwd. Blob resolution,
    // recovery and all journal writes remain after route validation below.
    let resumed_journal = resume_path
        .as_ref()
        .map(|path| SessionJournal::open(path).with_context(|| format!("opening session {}", path.display())))
        .transpose()?;
    if let Some(session_cwd) = resumed_journal
        .as_ref()
        .and_then(|journal| journal.header().get("cwd"))
        .and_then(|value| value.as_str())
        .map(PathBuf::from)
    {
        match &explicit_cwd {
            Some(c) if c != &session_cwd => eprintln!(
                "ara: warning: session was recorded in {} but tools run in --cwd {}",
                session_cwd.display(),
                c.display()
            ),
            Some(_) => {}
            None if session_cwd.is_dir() => cwd = session_cwd,
            None => {
                bail!("session cwd {} no longer exists; pass --cwd to choose where tools run", session_cwd.display())
            }
        }
    }
    let mut route = resolve_startup_route(&mut args, &cwd, account_auth, registry_cancel).await?;
    if args.mcp_config.is_none() && !args.mcp_allow.is_empty() {
        bail!("--mcp-allow requires --mcp-config");
    }
    let mcp_config =
        args.mcp_config.as_deref().map(McpServerConfig::from_file).transpose().map_err(anyhow::Error::msg)?;
    let mut prompts = args.prompts.clone();
    // Under `--repl`, stdin is the REPL's line input, not a prompt.
    if !args.repl
        && !rpc_mode
        && let Some(stdin) = read_stdin()?
    {
        // OMP buildInitialMessage: `${stdin}\n${firstPrompt}`.
        match prompts.first_mut() {
            Some(first) => *first = format!("{stdin}\n{first}"),
            None => prompts.push(stdin),
        }
    }
    let repl_mode = !rpc_mode && (args.repl || (prompts.is_empty() && std::io::stdin().is_terminal()));
    if prompts.is_empty() && !repl_mode && !rpc_mode {
        bail!("no prompt given (pass it as an argument or on stdin)");
    }
    let selected_api = if args.api() == Api::ProxyAuto {
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
        args.api()
    };

    // Create or mutate the journal only after validation above.
    let mut journal = if let Some(dir) = &session_dir {
        Some(match resumed_journal {
            Some(mut j) => {
                j.bind_blob_directory(&ara_blobs_directory())?;
                recover_session(&mut j)?;
                j
            }
            None => SessionJournal::create_with_blob_directory(dir, &cwd, &ara_blobs_directory())?,
        })
    } else {
        None
    };
    let model_ref = format!("{}/{}", route.model.provider, route.model.id);
    if let Some(j) = journal.as_mut()
        && j.current_model().as_deref() != Some(model_ref.as_str())
    {
        j.append_model_change(&model_ref)?;
    }
    let header = journal.as_ref().map(|j| j.header().clone()).unwrap_or_else(|| {
        if rpc_mode || selected_api == Api::OpenaiCodexResponses {
            ephemeral_header(&cwd, None)
        } else {
            serde_json::json!({"type":"session", "version":ara_session::CURRENT_SESSION_VERSION,
                "id":"ephemeral", "cwd":cwd.to_string_lossy()})
        }
    });
    let session_path = journal.as_ref().map(|j| j.path().to_path_buf());

    let setup = prepare_cli_setup(&args, &cwd, &route.model, &mcp_config, None, ToolOverlay::default()).await?;
    let CliSetup { system_prompt, tools, hooks, skills, tool_context } = setup;

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
    if matches!(args.api(), Api::ProxyAuto | Api::OpenaiCodexResponses) {
        client_builder = client_builder.redirect(reqwest::redirect::Policy::none());
    }
    let client = client_builder.build().context("building HTTP client")?;
    let cancel = CancellationToken::new();
    let provider_factory = if let Some(mut selection) = route.daily.take() {
        if let ara_cli::model_route::ProtocolOptions::Completions(options) = &mut selection.protocol {
            options.request_text_observer = stream_options.request_text_observer.take();
        }
        ProviderFactory::daily(
            client,
            selection,
            header.get("id").and_then(serde_json::Value::as_str).map(str::to_owned),
            &cwd,
            &cancel,
            account_auth.clone(),
            route.registry.clone(),
        )
        .await?
    } else {
        ProviderFactory::startup(
            client,
            route.model.clone(),
            selected_api,
            stream_options,
            args.anthropic_strict_tools,
            args.responses_stateful,
        )?
    };
    let mut context =
        journal.as_ref().map(|journal| provider_factory.context_for(journal)).transpose()?.unwrap_or_default();
    let mut provider = provider_factory.build();
    if let Some(account) = &provider_factory.account_auth {
        *account_auth = Some(account.clone());
    }
    // Match fixed main.ts: start discovery after Session/Host construction.
    // Catalog refresh remains background work; normal print shutdown drains
    // owned authentication and config helpers without awaiting the catalog.
    if let Some(registry) = &route.registry {
        registry.runtime().refresh_in_background(ara_cli::model_manager::ModelRefreshStrategy::OnlineIfUncached);
    }
    if rpc_mode {
        let config = AgentConfig {
            model: route.model,
            provider,
            system_prompt,
            tools,
            tool_choice: None,
            max_tokens: args.max_tokens,
            temperature: args.temperature,
            deadline: None,
            max_model_calls: args.max_model_calls,
            hooks,
        };
        let max_time = args.max_time;
        let sessions = rpc_host::SessionFactory {
            dir: session_dir,
            cwd,
            provider: provider_factory,
            args,
            mcp_config,
            tool_context,
            uri_port: None,
        };
        return rpc_host::run(config, skills, context, journal, header, max_time, sessions).await;
    }
    let journal = Some(match journal {
        Some(journal) => journal,
        None => SessionJournal::in_memory(header.clone())?,
    });
    let artifact_router = ara_cli::session_artifacts::ArtifactUriRouter::new(
        ara_cli::session_artifacts::SessionArtifacts::for_journal(journal.as_ref().expect("Session initialized")),
        None,
    );
    if let Some(context) = &tool_context {
        context.set_uri_port(artifact_router.clone());
    }
    let sink = HostSink::new(
        args.mode,
        repl_mode && args.mode == Mode::Text,
        journal,
        cancel.clone(),
        args.edit_mode,
        artifact_router.clone(),
    )
    .with_observed_usage(provider_factory.shared_account.clone());
    if args.mode == Mode::Json {
        sink.write_line(&header.to_string());
    }

    let mut interrupts = listen_for_interrupts()?;
    if repl_mode {
        let session = ReplSession {
            dir: session_dir,
            cwd: &cwd,
            model_ref: &model_ref,
            skills: &skills,
            provider_factory: &provider_factory,
            mcp_config: &mcp_config,
            artifact_router,
        };
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
    let mut last_printable_assistant = None;
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
        // Provider-native compaction replaces the model view with an opaque
        // replay carrier. Print the actual completed response, not that carrier.
        last_printable_assistant = context.iter().rev().find_map(Message::as_assistant).cloned();
        end = report.end;
        if end != RunEnd::Completed {
            break;
        }
        match run_compaction(&args, &config, &provider_factory, &mut context, &sink, &cancel, false).await {
            Ok(true) => {
                let id = sink.journal.lock().await.as_ref().context("Session unavailable")?.session_id().to_owned();
                provider = provider_factory.route.bind_codex_session(provider_factory.client.clone(), id);
            }
            Ok(false) => {}
            Err(e) => eprintln!("ara: auto-compaction failed ({e:#}); inspect the Session receipt"),
        }
        if sink.persist_failed.load(Ordering::SeqCst) {
            break;
        }
    }
    drop(provider_factory);
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
    let last = last_printable_assistant.or_else(|| context.iter().rev().find_map(Message::as_assistant).cloned());
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
    ara_cli::catalog_discovery::initialize_catalog_process_startup_tls();
    let args = Args::parse();
    let code = match Box::pin(run(args)).await {
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
        let artifacts = ara_cli::session_artifacts::ArtifactUriRouter::new(
            ara_cli::session_artifacts::SessionArtifacts::for_journal(&j),
            None,
        );
        let sink = HostSink::new(
            Mode::Text,
            false,
            Some(j),
            CancellationToken::new(),
            ara_edit::EditMode::Hashline,
            artifacts,
        );
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
