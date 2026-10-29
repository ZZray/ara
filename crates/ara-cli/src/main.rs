#![recursion_limit = "256"]

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
use ara_agent::{
    AgentConfig, AgentEvent, AgentEventSink, AgentMessage, LoopHooks, RawContextError, RunEnd,
    agent_loop_raw,
};

mod edit_mode_policy;
mod host_edit_post_apply;
mod host_thinking;
mod native_setup;
mod child_host;
mod peer_host;
mod skill_command;
mod startup_selection;
use ara_ai::providers::openai_completions::{
    PreparedRequestTextObservation, RequestTextObserver, StreamOptions,
};
use ara_ai::providers::openai_responses::StreamOptions as ResponsesStreamOptions;
use ara_ai::{
    AssistantMessageEvent, Message, Model, ModelProvider, ModelTokenizer, StopReason, UserMessage,
};
use ara_context::{
    DateCwdReminder, InternalUrls, PromptTool, SystemPromptOptions, build_system_prompt,
    resolve_prompt_input,
};
use ara_discovery::{Discovery, HostDirs, ProviderPolicy, SkillSourceSwitches, SkillsSettings};
use ara_mcp::ServerConfig as McpServerConfig;
use ara_session::{SessionJournal, latest_session};
use ara_tools::{ToolContext, builtin_tools};
use async_trait::async_trait;
use clap::{Parser, Subcommand, ValueEnum};
use std::future::Future;
use std::io::{IsTerminal, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

mod acp_host;
mod acp_plan;
mod acp_transport;
mod broker_management_host;
mod proxy_discovery;
mod rpc_host;
mod rpc_host_retry;
mod rpc_host_settings;
mod rpc_host_tools;
mod rpc_host_uris;
mod session_quota_recovery;

const TOOL_NAMES: [&str; 12] = [
    "read",
    "write",
    "edit",
    "bash",
    "grep",
    "glob",
    "ast_grep",
    "ast_edit",
    "debug",
    "web_search",
    "wait",
    "task",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Mode {
    /// Final assistant text only.
    Text,
    /// Every agent event as one JSON line.
    Json,
    /// JSONL RPC commands and live Agent events.
    Rpc,
    /// Agent Client Protocol over bidirectional NDJSON JSON-RPC stdio.
    Acp,
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
#[command(
    name = "ara",
    version,
    about = "ARA agent (print mode, or a line REPL on a terminal)"
)]
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
    /// Ordered model scope patterns, comma separated. Repeating uses the last value.
    #[arg(long, action = clap::ArgAction::Append)]
    models: Vec<String>,
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
    /// Thinking selector: off, auto, minimal, low, medium, high, xhigh or max.
    #[arg(long)]
    thinking: Vec<String>,
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
    /// Tools to enable (comma separated): read,write,edit,bash,grep,glob,ast_grep,ast_edit. Empty disables tools.
    #[arg(long)]
    tools: Option<String>,
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
    #[arg(long, value_parser = parse_edit_mode)]
    edit_mode: Option<ara_edit::EditMode>,
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
    /// Manage the local or remote credential broker.
    AuthBroker(ara_cli::auth_broker_management::BrokerArgs),
    /// Sign in to OpenAI in a browser, or explicitly use a device code.
    Login {
        #[arg(default_value = "openai-codex")]
        provider: String,
        #[arg(long, value_enum, default_value = "browser")]
        method: LoginMethod,
    },
    /// Remove the saved OpenAI account credentials.
    Logout {
        #[arg(default_value = "openai-codex")]
        provider: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum LoginMethod {
    Browser,
    Device,
}

fn parse_edit_mode(value: &str) -> Result<ara_edit::EditMode, String> {
    ara_edit::EditMode::parse(value).ok_or_else(|| {
        format!("unknown edit mode {value:?} (hashline, replace, patch, apply_patch, sloppy)")
    })
}

/// Per-request provider context rewrite: the date/cwd reminder on the first
/// user turn (OMP `DateCwdReminderInjector`).
struct CliHooks {
    peer_ingress: Option<Arc<peer_host::PeerIngress>>,
    child_contract:Option<Arc<ara_cli::child_contract::ChildContractOwner>>,
    child_yield:Option<Arc<ara_cli::child_contract::NativeYieldBundle>>,
    edit_session: Option<Arc<ara_cli::host_edit_session::HostEditSession>>,
    edit_native_session: Option<Arc<ara_cli::native_factory::PreparedNativeSession>>,
    service_session: Option<Arc<ara_cli::host_services::SessionServiceLease>>,
    reminder: DateCwdReminder,
    actual_reminder: ara_cli::actual_host::GraphDateCwdReminder,
    cwd: String,
    context_controller: ara_cli::append_only_context_mode::SessionContextController,
    model: Model,
    projector: Arc<ara_cli::agent_message_projection::AgentMessageProjector>,
    projection_journal: Arc<tokio::sync::Mutex<Option<SessionJournal>>>,
    native_tool_hooks: Option<Arc<ara_cli::actual_tools::NativeBuiltinToolHooks>>,
    bind_native_tool_context: bool,
    pending_resolution: ara_tools::resolve::PendingResolutionQueue,
}

impl CliHooks {
    fn native_tool_hooks(
        &self,
        values: &ara_agent::SessionValues,
    ) -> Result<&ara_cli::actual_tools::NativeBuiltinToolHooks, ara_agent::ActualProviderError>
    {
        let hooks = self.native_tool_hooks.as_deref().ok_or(
            ara_agent::ActualProviderError::Unsupported(
                "CLI same-Session Native builtin hooks are not connected",
            ),
        )?;
        hooks.check_values(values)?;
        Ok(hooks)
    }
}

#[async_trait]
impl LoopHooks for CliHooks {
    fn irc_interrupt_port(&self) -> Option<Arc<dyn ara_agent::IrcInterruptPeek>> {
        self.peer_ingress.as_ref().map(|ingress| ingress.clone() as Arc<dyn ara_agent::IrcInterruptPeek>)
    }
    async fn aside_messages(
        &self,
    ) -> std::result::Result<Vec<ara_agent::AsideMessage>, ara_agent::AsideError> {
        let mut entries = self.edit_session.as_ref()
            .map(|owner| owner.asides(self.projector.clone(), self.edit_native_session.clone()))
            .unwrap_or_default();
        if let Some(service) = &self.service_session {
            entries.extend(service.asides(self.projector.clone(), self.edit_native_session.clone()));
        }
        if let Some(ingress) = &self.peer_ingress { entries.extend(ingress.asides(self.projector.clone(), self.edit_native_session.clone())?); }
        Ok(entries)
    }
    fn select_tool_choice_directive_full(
        &self,
        active_tools: &[Arc<dyn ara_agent::AgentTool>],
        admitted: Option<&ara_ai::ToolChoice>,
    ) -> Result<
        ara_agent::ToolChoiceDirectiveSelection,
        ara_agent::tool_directives::ToolDirectiveError,
    > {
        let requirement = if active_tools
            .iter()
            .any(|tool| tool.definition().name == "write")
        {
            self.pending_resolution
                .soft_requirement_with(|body| self.projector.new_legacy_message(body, None))
                .map_err(|error| {
                    ara_agent::tool_directives::ToolDirectiveError(error.to_string())
                })?
        } else {
            None
        };
        Ok(ara_agent::ToolChoiceDirectiveSelection {
            directive: requirement.map(ara_agent::ToolChoiceDirective::Soft),
            static_fallback: admitted.cloned(),
            suppress_static_soft_block: false,
        })
    }
    fn soft_tool_requirement_state(&self) -> Option<ara_agent::SoftToolRequirementState> {
        Some(self.pending_resolution.soft_state())
    }
    fn actual_tool_source(
        &self,
        values: &ara_agent::SessionValues,
    ) -> Result<Option<Arc<dyn ara_agent::ActualToolSource>>, ara_agent::ActualProviderError> {
        match &self.native_tool_hooks {
            Some(hooks) => hooks.source(values).map(Some),
            None => Ok(None),
        }
    }
    fn supports_actual_tool_hooks(&self) -> bool {
        self.native_tool_hooks.is_some()
    }
    async fn before_tool_dispatch_actual(
        &self,
        context: ara_agent::ActualBeforeToolCallContext<'_>,
        _cancel: &CancellationToken,
    ) -> Result<Option<ara_rpc::value_heap::ValueHandle>, ara_agent::ActualProviderError> {
        self.native_tool_hooks(context.values)?;
        context.values.check_handle(context.tool_call)?;
        context.values.check_handle(context.args)?;
        if context.tool.capability.definition().name == "read" {
            self.native_tool_hooks(context.values)?
                .capture_read_attachments(
                    context.context.messages,
                    ara_blobs_directory(),
                    context.values,
                )?;
        }
        if let Some(bundle)=&self.child_yield {bundle.before_tool_dispatch(&context)?;}
        // This concrete CLI type has no before-dispatch extension callback.
        Ok(None)
    }
    async fn before_tool_call_actual(
        &self,
        context: ara_agent::ActualBeforeToolCallContext<'_>,
        _cancel: &CancellationToken,
    ) -> Result<ara_agent::ActualToolDecision, ara_agent::ActualProviderError> {
        self.native_tool_hooks(context.values)?
            .permission(context.tool, context.values)
    }
    async fn after_tool_call_actual(
        &self,
        context: ara_agent::ActualAfterToolCallContext<'_>,
        _cancel: &CancellationToken,
    ) -> Result<Option<ara_rpc::value_heap::ValueHandle>, ara_agent::ActualProviderError> {
        self.native_tool_hooks(context.values)?;
        context.values.check_handle(context.tool_call)?;
        context.values.check_handle(context.args)?;
        context.values.check_handle(context.result)?;
        if let Some(bundle)=&self.child_yield {bundle.after_tool_call(&context)?;}
        // This concrete CLI type has no after-tool extension callback.
        Ok(None)
    }
    fn transform_tool_call_arguments_actual(
        &self,
        args: ara_rpc::value_heap::ValueHandle,
        _name: &ara_rpc::WireString,
        values: &ara_agent::SessionValues,
    ) -> Result<ara_rpc::value_heap::ValueHandle, ara_agent::ActualProviderError> {
        self.native_tool_hooks(values)?;
        values.check_handle(&args)?;
        // CliHooks has no typed argument transformer: retain the exact argument.
        Ok(args)
    }
    fn get_tool_context_actual(
        &self,
        context: &ara_agent::ActualToolExecutionContext,
        values: &ara_agent::SessionValues,
    ) -> Result<ara_rpc::value_heap::ValueHandle, ara_agent::ActualProviderError> {
        let hooks = self.native_tool_hooks(values)?;
        let fresh = hooks.tool_context(context, values)?;
        // RPC's ToolLoopHooks owns admission there; CLI/REPL admit at creation.
        if self.bind_native_tool_context {
            hooks.bind_run_context(values, &fresh)?;
        }
        Ok(fresh)
    }
    fn tool_error_message_actual(
        &self,
        error: &ara_rpc::value_heap::ValueError,
        values: &ara_agent::SessionValues,
    ) -> Result<ara_rpc::value_heap::ValueHandle, ara_agent::ActualProviderError> {
        self.native_tool_hooks(values)?.error_message(error, values)
    }
    fn sanitize_tool_result_text_actual(
        &self,
        text: &ara_rpc::value_heap::ValueHandle,
        values: &ara_agent::SessionValues,
    ) -> Result<ara_rpc::value_heap::ValueHandle, ara_agent::ActualProviderError> {
        self.native_tool_hooks(values)?.sanitize(text, values)
    }
    fn soft_tool_satisfies_actual(
        &self,
        requirement: &ara_agent::soft_tool_requirement::SoftToolRequirement,
        call: &ara_rpc::value_heap::ValueHandle,
        values: &ara_agent::SessionValues,
    ) -> Result<bool, ara_agent::ActualProviderError> {
        self.native_tool_hooks(values)?
            .soft_satisfies(requirement, call, values)
    }
    fn provider_context_binding(&self) -> Option<ara_agent::ProviderContextBinding> {
        Some(self.context_controller.binding())
    }
    fn set_append_only_context(
        &self,
        manager: Option<ara_agent::SharedAppendOnlyContextManager>,
    ) -> std::result::Result<(), ara_agent::ProviderContextError> {
        self.context_controller.set_append_only_context(manager);
        Ok(())
    }

    fn queued_input_pending(&self, _kind: ara_agent::QueueInputKind) -> Option<bool> {
        // This hook only rewrites context; the Agent owns the input queues.
        Some(false)
    }

    async fn convert_agent_messages_to_llm(
        &self,
        messages: &[AgentMessage],
    ) -> Result<Vec<Message>, RawContextError> {
        let journal = self.projection_journal.lock().await;
        if let Some(journal) = journal.as_ref() {
            self.projector
                .refresh_checked(journal, messages, &self.model)?;
        }
        drop(journal);
        self.projector.convert_current(messages, &self.model)
    }

    async fn transform_provider_context(
        &self,
        context: ara_ai::Context,
        _model: &Model,
    ) -> ara_ai::Context {
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        self.reminder.transform(context, &date, &self.cwd)
    }

    async fn convert_agent_messages_to_llm_actual(
        &self,
        messages: &[AgentMessage],
        values: &ara_agent::SessionValues,
    ) -> Result<Vec<ara_rpc::value_heap::ValueHandle>, ara_agent::ActualProviderError> {
        let journal = self.projection_journal.lock().await;
        if let Some(journal) = journal.as_ref() {
            self.projector
                .refresh_checked(journal, messages, &self.model)
                .map_err(|error| ara_agent::ActualProviderError::Callback(error.to_string()))?;
        }
        drop(journal);
        self.projector
            .convert_current_actual(messages, &self.model, values)
    }
    async fn transform_provider_context_actual(
        &self,
        context: ara_agent::ActualProviderContext,
        _model: &Model,
        values: &ara_agent::SessionValues,
    ) -> Result<ara_agent::ActualProviderContext, ara_agent::ActualProviderError> {
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        self.actual_reminder
            .transform(context, values, &date, &self.cwd)
    }
    async fn transform_assistant_graph(
        &self,
        _owner: &AgentMessage,
        _cancel: &CancellationToken,
        _values: &ara_agent::SessionValues,
    ) -> Result<(), ara_agent::ActualProviderError> {
        // CliHooks has no typed transform-assistant hook either.
        Ok(())
    }
    fn supports_actual_turn_end(&self) -> bool {
        true
    }
    async fn turn_end_actual(
        &self,
        _owner: &AgentMessage,
        _context: &[AgentMessage],
        _cancel: &CancellationToken,
        _values: &ara_agent::SessionValues,
    ) -> Result<Vec<AgentMessage>, ara_agent::ActualProviderError> {
        // This concrete CLI hook has no turn-end callback; wrappers retain
        // their independently required callback consumers.
        Ok(Vec::new())
    }
}

/// `$ARA_HOME` (default `~/.ara`), absolute against the process cwd so paths
/// derived from it do not depend on the session's `--cwd`.
fn ara_home() -> PathBuf {
    let home = std::env::var_os("ARA_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(|h| PathBuf::from(h).join(".ara"))
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
        let control =
            (code <= 0x08) || (0x0B..=0x1F).contains(&code) || (0x7F..=0x9F).contains(&code);
        if !control {
            out.push(c);
        }
    }
    out
}

fn env_first(names: &[&str]) -> Option<(String, String)> {
    names.iter().find_map(|n| {
        std::env::var(n)
            .ok()
            .filter(|v| !v.is_empty())
            .map(|v| (n.to_string(), v))
    })
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
        url.scheme() == "https"
            && url
                .host_str()
                .is_some_and(|host| host.eq_ignore_ascii_case("api.anthropic.com"))
    })
}

fn default_session_dir(cwd: &Path) -> PathBuf {
    let home = std::env::var_os("ARA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".ara")))
        .unwrap_or_else(|| PathBuf::from(".ara"));
    let encoded = format!(
        "--{}--",
        cwd.to_string_lossy()
            .trim_matches('/')
            .replace(['/', '\\', ':', '?'], "-")
    );
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
    console_task: std::sync::Mutex<Option<Arc<task_print::ConsoleTaskHost>>>,
    wait_session: Option<Arc<ara_cli::actual_tools::wait_native::SessionWaitOwner>>,
    service_session: Option<Arc<ara_cli::host_services::SessionServiceLease>>,
    edit_session: Option<Arc<ara_cli::host_edit_session::HostEditSession>>,
    mode: Mode,
    /// REPL text mode: stream assistant text to stdout and report tool
    /// progress on stderr while the turn runs.
    stream: bool,
    terminal: Option<Arc<ara_cli::terminal::TerminalDriver>>,
    terminal_generation: std::sync::atomic::AtomicU64,
    display_revision: std::sync::atomic::AtomicU64,
    collapse_compacted: bool,
    display: Option<Arc<tokio::sync::Mutex<ara_cli::terminal::evaluation::DisplayEvaluator>>>,
    /// A streamed text line is open on stdout (no trailing newline yet).
    line_open: AtomicBool,
    /// The current assistant message has streamed text.
    streamed: AtomicBool,
    journal: Arc<tokio::sync::Mutex<Option<SessionJournal>>>,
    projector: Arc<ara_cli::agent_message_projection::AgentMessageProjector>,
    artifacts: Arc<ara_cli::session_artifacts::ArtifactUriRouter>,
    persist_failed: AtomicBool,
    /// Auto-compaction already said why it cannot run; it says so once per process.
    compaction_notice_shown: AtomicBool,
    cancel: CancellationToken,
    /// How `edit` payloads are written, to name their target files in progress lines.
    edit_mode: std::sync::RwLock<ara_edit::EditMode>,
    observed_usage: Option<Arc<ara_cli::auth_storage::AuthStorage>>,
}

struct HostTerminalRun<'a> {
    sink: &'a HostSink,
    terminal: Arc<ara_cli::terminal::TerminalDriver>,
    generation: u64,
}
impl Drop for HostTerminalRun<'_> {
    fn drop(&mut self) {
        self.sink.terminal_generation.store(0, Ordering::Release);
        if let Err(error) = self.terminal.end_run(self.generation) {
            eprintln!("ara: terminal restoration failed ({error})");
        }
    }
}

impl HostSink {
    fn task_host(&self)->Option<Arc<task_print::ConsoleTaskHost>>{self.console_task.lock().unwrap_or_else(|e|e.into_inner()).clone()}

    async fn refresh_terminal_history(
        &self,
        native: Option<Arc<ara_cli::native_factory::PreparedNativeSession>>,
    ) -> Result<()> {
        use ara_cli::terminal::{
            evaluation::Admission, session_observation::SessionTranscriptObservation,
        };
        use ara_session::context_view::{TranscriptViewMode, TranscriptViewOptions};
        let Some(display) = &self.display else {
            return Ok(());
        };
        {
            let mut owner = display.lock().await;
            owner.attach_native(native.clone())?;
            owner.pump_quantum();
        }
        let observation = {
            let guard = self.journal.lock().await;
            let Some(journal) = guard.as_ref() else {
                return Ok(());
            };
            let view = journal.transcript_view(TranscriptViewOptions {
                mode: if self.collapse_compacted {
                    TranscriptViewMode::Collapsed
                } else {
                    TranscriptViewMode::Full
                },
                keep_dangling_tool_calls: false,
            })?;
            let mut originals = std::collections::HashMap::new();
            let entries: std::collections::HashMap<_, _> = journal
                .entries()
                .iter()
                .map(|entry| (entry.id.as_str(), entry))
                .collect();
            for row in &view.messages {
                if let Some(entry) = entries.get(row.entry_id.as_str()) {
                    if let Some(body) = entry.agent_message_body()? {
                        let original = self
                            .projector
                            .message_from_body(body, entry.original_source())?;
                        originals.insert(row.entry_id.clone(), original);
                    }
                }
            }
            SessionTranscriptObservation {
                owner: native,
                session_id: journal.session_id().to_owned(),
                revision: self
                    .display_revision
                    .fetch_add(1, Ordering::AcqRel)
                    .wrapping_add(1),
                collapsed: self.collapse_compacted,
                view,
                originals,
            }
        }; // No Journal lock crosses display queue or rendering.
        let port = self
            .terminal
            .as_ref()
            .expect("display owns driver")
            .display_port();
        match port.admit_snapshot(observation) {
            Admission::Accepted(_) => {
                display.lock().await.pump_quantum();
            }
            Admission::Full => self
                .terminal
                .as_ref()
                .unwrap()
                .diagnostic("ara: Session display snapshot admission full"),
            Admission::Closed => {}
        }
        Ok(())
    }
    fn set_edit_mode(&self, mode: ara_edit::EditMode) {
        *self
            .edit_mode
            .write()
            .unwrap_or_else(|error| error.into_inner()) = mode;
    }

    fn persistence_failure(&self, error: &impl std::fmt::Display) {
        if let Some(service) = &self.service_session { service.close(); }
        self.persist_failed.store(true, Ordering::SeqCst);
        ara_cli::terminal::report_diagnostic(&format!(
            "ara: session persistence failed ({error}); aborting the run"
        ));
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
        let terminal = if stream {
            ara_cli::terminal::TerminalDriver::for_stdio(edit_mode)
        } else {
            None
        };
        let display = terminal.as_ref().map(|driver| {
            Arc::new(tokio::sync::Mutex::new(
                ara_cli::terminal::evaluation::DisplayEvaluator::new(
                    driver.clone(),
                    None,
                    edit_mode,
                )
                .expect("owned terminal initialization has no Native admission"),
            ))
        });
        HostSink {
            console_task: std::sync::Mutex::new(None),
            wait_session: None,
            service_session: None,
            edit_session: None,
            mode,
            stream,
            terminal,
            display,
            terminal_generation: std::sync::atomic::AtomicU64::new(0),
            display_revision: std::sync::atomic::AtomicU64::new(0),
            collapse_compacted: true,
            line_open: AtomicBool::new(false),
            streamed: AtomicBool::new(false),
            journal: Arc::new(tokio::sync::Mutex::new(journal)),
            projector: Arc::new(
                ara_cli::agent_message_projection::AgentMessageProjector::default(),
            ),
            artifacts,
            persist_failed: AtomicBool::new(false),
            compaction_notice_shown: AtomicBool::new(false),
            cancel,
            edit_mode: std::sync::RwLock::new(edit_mode),
            observed_usage: None,
        }
    }

    fn with_terminal_presentation(
        mut self,
        settings: ara_cli::terminal::render_utils::TerminalPresentationSettings,
    ) -> Self {
        self.collapse_compacted = settings.collapse_compacted;
        // The new sink is not published yet; no event can own this evaluator.
        if let Some(display) = &self.display {
            display
                .try_lock()
                .expect("unpublished display has no concurrent owner")
                .set_presentation(settings);
        }
        self
    }
    fn with_projection_owner(
        mut self,
        projector: Arc<ara_cli::agent_message_projection::AgentMessageProjector>,
        journal: Arc<tokio::sync::Mutex<Option<SessionJournal>>>,
    ) -> Self {
        self.projector = projector;
        self.journal = journal;
        self
    }

    fn with_service_session(mut self, lease: Option<Arc<ara_cli::host_services::SessionServiceLease>>) -> Self {
        self.service_session = lease;
        self
    }
    fn with_wait_session(mut self, owner: Option<Arc<ara_cli::actual_tools::wait_native::SessionWaitOwner>>) -> Self {
        self.wait_session = owner;
        self
    }
    fn wait_owner(&self)->Option<Arc<ara_cli::actual_tools::wait_native::SessionWaitOwner>>{self.wait_session.clone()}
    async fn bind_service_session(&self) -> Result<()> {
        if let Some(wait)=self.wait_owner(){let guard=self.journal.lock().await;let journal=guard.as_ref().context("Wait requires current SessionJournal")?;wait.bind_session(journal.session_id(),journal.is_persistent())?;}
        if let Some(service) = &self.service_session {
            let guard = self.journal.lock().await;
            let journal = guard.as_ref().context("Service completion requires current SessionJournal")?;
            service.bind(journal.session_id(), journal.is_persistent())?;
        }
        Ok(())
    }
    fn with_edit_session(
        mut self,
        owner: Option<Arc<ara_cli::host_edit_session::HostEditSession>>,
    ) -> Self {
        self.edit_session = owner;
        self
    }

    fn with_observed_usage(
        mut self,
        owner: Option<Arc<ara_cli::auth_storage::AuthStorage>>,
    ) -> Self {
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

    fn stream_progress(&self, event: &AgentEvent) -> Result<(), ara_agent::RawContextError> {
        match event {
            AgentEvent::MessageStart { .. } => self.streamed.store(false, Ordering::SeqCst),
            AgentEvent::MessageUpdate {
                event: AssistantMessageEvent::TextStart { .. },
                ..
            }
            | AgentEvent::ActualMessageUpdate {
                event: AssistantMessageEvent::TextStart { .. },
                ..
            } => self.end_stream_line(),
            AgentEvent::MessageUpdate {
                event: AssistantMessageEvent::TextDelta { delta, .. },
                ..
            } => {
                let text = sanitize_text(delta);
                if !text.is_empty() {
                    let mut out = std::io::stdout().lock();
                    let _ = write!(out, "{text}");
                    let _ = out.flush();
                    self.line_open
                        .store(!text.ends_with('\n'), Ordering::SeqCst);
                    self.streamed.store(true, Ordering::SeqCst);
                }
            }
            AgentEvent::ActualMessageUpdate {
                event: AssistantMessageEvent::TextDelta { delta, .. },
                ..
            } => {
                let delta = delta.to_utf8().map_err(|error| {
                    ara_agent::RawContextError(format!(
                        "Actual text display requires Unicode scalar source: {error}"
                    ))
                })?;
                let text = sanitize_text(&delta);
                if !text.is_empty() {
                    let mut out = std::io::stdout().lock();
                    let _ = write!(out, "{text}");
                    let _ = out.flush();
                    self.line_open
                        .store(!text.ends_with('\n'), Ordering::SeqCst);
                    self.streamed.store(true, Ordering::SeqCst);
                }
            }
            AgentEvent::MessageEnd { message } | AgentEvent::ActualMessageEnd { message, .. } => {
                let Some(a) = self.projector.display_assistant(message)? else {
                    return Ok(());
                };
                self.end_stream_line();
                // A provider that delivered its text without deltas still shows it.
                if !self.streamed.swap(false, Ordering::SeqCst) {
                    for block in &a.content {
                        if let ara_cli::actual_host::AssistantDisplayBlock::Text(text) = block
                            && !text.is_empty()
                        {
                            self.write_line(&sanitize_text(text));
                        }
                    }
                }
            }
            AgentEvent::ToolExecutionStart {
                tool_name, args, ..
            } => {
                self.end_stream_line();
                eprintln!(
                    "ara: tool {}{}",
                    sanitize_text(tool_name),
                    tool_summary(
                        tool_name,
                        args,
                        *self
                            .edit_mode
                            .read()
                            .unwrap_or_else(|error| error.into_inner())
                    )
                );
            }
            AgentEvent::ActualToolExecutionStart {
                tool_name, args, ..
            } => {
                self.end_stream_line();
                let name = tool_name.to_utf8().map_err(|error| {
                    RawContextError(format!("Actual tool display string: {error}"))
                })?;
                let mut named = None;
                for key in ["command", "path", "pattern", "pat"] {
                    named = self.projector.activity_field(args, key)?;
                    if named.is_some() {
                        break;
                    }
                }
                if named.is_none() && name == "edit" {
                    named = self
                        .projector
                        .activity_field(args, "input")?
                        .and_then(|input| {
                            edit_input_target(
                                *self
                                    .edit_mode
                                    .read()
                                    .unwrap_or_else(|error| error.into_inner()),
                                &input,
                            )
                        });
                }
                let summary = named
                    .map(|value| {
                        let flat = sanitize_text(&value)
                            .split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" ");
                        let cut: String = flat.chars().take(120).collect();
                        if cut.len() < flat.len() {
                            format!(": {cut}...")
                        } else {
                            format!(": {cut}")
                        }
                    })
                    .unwrap_or_default();
                eprintln!("ara: tool {}{}", sanitize_text(&name), summary);
            }
            AgentEvent::ActualToolExecutionEnd {
                tool_name,
                is_error,
                ..
            } => {
                let name = tool_name.to_utf8().map_err(|error| {
                    RawContextError(format!("Actual tool display string: {error}"))
                })?;
                eprintln!(
                    "ara: tool {} {}",
                    sanitize_text(&name),
                    if *is_error { "failed" } else { "done" }
                );
            }
            AgentEvent::ToolExecutionEnd {
                tool_name,
                is_error,
                ..
            } => {
                eprintln!(
                    "ara: tool {} {}",
                    sanitize_text(tool_name),
                    if *is_error { "failed" } else { "done" }
                );
            }
            _ => {}
        }
        Ok(())
    }
}

/// One-line summary of a tool call for REPL progress: its command, path or
/// pattern, with whitespace folded and cut to 120 characters. An `edit` whose
/// payload is one `input` names its target file instead, as OMP's
/// `editToolRenderer.activitySummary` does.
fn tool_summary(
    tool_name: &str,
    args: &ara_ai::JsonObject,
    edit_mode: ara_edit::EditMode,
) -> String {
    let named = ["command", "path", "pattern", "pat"]
        .iter()
        .find_map(|k| args.get(*k).and_then(serde_json::Value::as_str))
        .map(str::to_owned);
    let edit_target = || {
        let input = args.get("input").and_then(serde_json::Value::as_str)?;
        (tool_name == "edit")
            .then(|| edit_input_target(edit_mode, input))
            .flatten()
    };
    let Some(value) = named.or_else(edit_target) else {
        return String::new();
    };
    let flat = sanitize_text(&value)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let cut: String = flat.chars().take(120).collect();
    if cut.len() < flat.len() {
        format!(": {cut}...")
    } else {
        format!(": {cut}")
    }
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
        EditMode::ApplyPatch => apply_patch::parse_apply_patch_streaming(input)
            .ok()?
            .into_iter()
            .map(|e| e.path)
            .collect(),
        EditMode::Sloppy => sloppy::parse::split_sloppy_sections(input)
            .into_iter()
            .map(|s| s.path)
            .collect(),
        EditMode::Replace | EditMode::Patch => return None,
    };
    let first = paths.first()?;
    Some(if paths.len() > 1 {
        format!("{first} (+{} more)", paths.len() - 1)
    } else {
        first.clone()
    })
}

impl Drop for HostSink {
    fn drop(&mut self) { if let Some(service) = &self.service_session { service.close(); } }
}

#[async_trait]
impl AgentEventSink for HostSink {
    async fn emit(&self, event: AgentEvent) {
        if self.persist_failed.load(Ordering::SeqCst) {
            return;
        }
        // Preserve the existing legacy fail-stop boundary before journal I/O.
        // Actual Graph bodies keep their native append owner and are derived later.
        let projected = match &event {
            AgentEvent::MessageEnd { message } | AgentEvent::ActualMessageEnd { message, .. }
                if !matches!(
                    message.body_snapshot(),
                    ara_agent::AgentMessageBody::Graph(_)
                ) =>
            {
                match message.checked_model_snapshot() {
                    Ok(projected) => projected,
                    Err(error) => {
                        self.persistence_failure(&error);
                        return;
                    }
                }
            }
            _ => None,
        };
        let legacy_printable =
            if self.mode == Mode::Json && self.projector.actual_values().is_none() {
                match event.printable() {
                    Ok(value) => Some(value),
                    Err(error) => {
                        self.persistence_failure(&error);
                        return;
                    }
                }
            } else {
                None
            };
        let mut persisted_entry = None;
        if let AgentEvent::MessageEnd { message } | AgentEvent::ActualMessageEnd { message, .. } =
            &event
        {
            // Read the weak claim before taking the journal lock. Ordinary
            // messages and --no-session retain their existing lazy behavior.
            let durable_service = self.service_session.as_ref()
                .is_some_and(|service| service.requires_durable_append(message));
            let wait_owner=self.wait_owner();
            let durable_wait=wait_owner.as_ref().is_some_and(|wait|wait.requires_durable_append(message));
            let peer_ingress=self.task_host().map(|host|host.ingress());
            let durable_peer=peer_ingress.as_ref().is_some_and(|ingress|ingress.is_parent_claim(message));
            let mut append_identity = None;
            let appended = {
                let mut guard = self.journal.lock().await;
                guard.as_mut().map(|journal| -> ara_session::Result<String> {
                    let id = self.projector.append_current_body(journal, message)?;
                    if (durable_service||durable_wait||durable_peer) && journal.is_persistent() && !journal.is_on_disk() {
                        // append_current_body can succeed while a fresh journal
                        // is still lazy. ACK needs the existing durable rewrite
                        // barrier, including its original IO/directory errors.
                        journal.materialize()?;
                    }
                    // If a claim arrived while awaiting this lock, a lazy
                    // journal still cannot masquerade as a durable receipt.
                    append_identity = Some((journal.session_id().to_owned(),
                        journal.is_persistent() && journal.is_on_disk()));
                    Ok(id)
                })
            };
            if let Some(service) = &self.service_session {
                let receipt = match (&appended, append_identity.clone()) {
                    (Some(Ok(id)), Some((session_id, persistent))) => Ok(ara_cli::host_services::CompletionAdoption { session_id, entry_id: id.clone(), persistent }),
                    (Some(Err(error)), _) => Err(error.to_string()),
                    _ => Err("Session journal unavailable before service completion adoption".into()),
                };
                service.append_receipt(message, receipt);
            }
            if let Some(wait)=wait_owner {
                let receipt=match(&appended,append_identity.clone()){(Some(Ok(id)),Some((session_id,persistent)))=>Ok(ara_cli::host_peers::ParentAdoption{session_id,entry_id:id.clone(),persistent}),(Some(Err(error)),_)=>Err(error.to_string()),_=>Err("Session journal unavailable before Native Wait result adoption".into())};
                if let Err(error)=wait.append_receipt(message,receipt){self.persistence_failure(&error.0);return;}
            }
            if let Some(ingress)=peer_ingress {
                let receipt=match(&appended,append_identity){
                    (Some(Ok(id)),Some((session_id,persistent)))=>Ok(ara_cli::host_peers::ParentAdoption{session_id,entry_id:id.clone(),persistent}),
                    (Some(Err(error)),_)=>Err(anyhow::anyhow!(error.to_string())),
                    _=>Err(anyhow::anyhow!("Console parent result lacks its original journal append receipt")),
                };
                ingress.append_parent_receipt(message,receipt);
            }
            match appended {
                Some(Ok(id)) if self.display.is_some() => {
                    // Original append receipt is obtained once, before display admission.
                    // Failure to observe display data never repeats journal I/O.
                    match self.projector.current_wire_body(message) {
                        Ok(body) => persisted_entry = Some((id, body)),
                        Err(error) => {
                            if let Some(terminal) = &self.terminal {
                                terminal.diagnostic(&format!(
                                    "ara: entry display observation failed ({error})"
                                ));
                            }
                        }
                    }
                }
                Some(Err(error)) => {
                    self.persistence_failure(&error);
                    return;
                }
                Some(Ok(_)) | None => {}
            }
        }
        if let AgentEvent::MessageEnd { message } | AgentEvent::ActualMessageEnd { message, .. } =
            &event
            && let Some(owner) = &self.observed_usage
        {
            match message.body_snapshot() {
                ara_agent::AgentMessageBody::Graph(_) => {
                    match self.projector.observed_actual_usage(message) {
                        Ok(Some(entry)) => owner.record_observed_usage(&[entry], None),
                        Ok(None) => {}
                        Err(error) => {
                            self.persistence_failure(&error);
                            return;
                        }
                    }
                }
                _ => {
                    if let Some(Message::Assistant(message)) = &projected {
                        owner.record_assistant_usage(message);
                    }
                }
            }
        }
        let printable = if legacy_printable.is_some() {
            legacy_printable
        } else if self.mode == Mode::Json {
            match self.projector.public_event(&event, false) {
                Ok(value) => Some(value),
                Err(error) => {
                    self.persistence_failure(&error);
                    return;
                }
            }
        } else {
            None
        };
        if self.stream {
            if let Some(terminal) = &self.terminal {
                // Journal/Core receipts have already been handled above. This
                // acknowledges display admission, not constructor completion.
                let receipt = terminal
                    .display_port()
                    .admit_persisted(
                        self.terminal_generation.load(Ordering::Acquire),
                        event.clone(),
                        persisted_entry,
                    )
                    .await;
                if matches!(receipt, ara_cli::terminal::evaluation::Admission::Full) {
                    terminal.diagnostic("ara: display admission full");
                }
            } else if let Err(error) = self.stream_progress(&event) {
                self.persistence_failure(&error);
                return;
            }
        }
        if let Some(printable) = printable {
            self.write_line(&printable.to_string());
        }
    }
}

fn last_assistant(
    messages: &[AgentMessage],
    projector: &ara_cli::agent_message_projection::AgentMessageProjector,
) -> Result<Option<ara_cli::actual_host::AssistantDisplay>, ara_agent::RawContextError> {
    for raw in messages.iter().rev() {
        if let Some(assistant) = projector.display_assistant(raw)? {
            return Ok(Some(assistant));
        }
    }
    Ok(None)
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
    startup: Option<startup_selection::StartupState>,
}

/// Validate every argument that does not need the journal (no I/O side effects).
fn validate_route_args(args: &Args) -> Result<()> {
    if args.api() == Api::ProxyAuto {
        if args.resume.is_some() || args.continue_session {
            bail!(
                "--api proxy-auto cannot resume a session; select an explicit --api and verify its endpoint"
            );
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
        if matches!(
            args.provider.as_deref(),
            Some("opencode-go" | "opencode-zen" | "umans")
        ) {
            bail!(
                "--api proxy-auto cannot use a provider label with a different authentication scheme"
            );
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
    for name in args
        .tools
        .as_deref()
        .unwrap_or("read,write,edit,bash,grep,glob,task")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if !TOOL_NAMES.contains(&name) {
            bail!(
                "unknown tool {name:?} (available: {})",
                TOOL_NAMES.join(", ")
            );
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
    let tokenizer_choice = args
        .tokenizer
        .clone()
        .or_else(|| env_first(&["ARA_TOKENIZER"]).map(|(_, value)| value));
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
        .or_else(|| {
            env_first(&["ARA_BASE_URL", "ARA_TEST_BASE_URL", "OPENROUTER_BASE_URL"]).map(|(_, v)| v)
        })
        .context("no base URL: pass --base-url or set ARA_BASE_URL")?;
    if args.api() == Api::ProxyAuto && is_official_anthropic(&base_url) {
        bail!("--api proxy-auto requires a dual-protocol proxy, not the official Anthropic route");
    }
    let openrouter = is_openrouter(&base_url);
    // Route-specific keys stay on their own hosts. ARA keys or an explicit
    // --api-key-env may be used for another endpoint.
    let api_key = match &args.api_key_env {
        Some(name) => Some(
            std::env::var(name)
                .with_context(|| format!("environment variable {name} is not set"))?,
        ),
        None if args.api() == Api::AnthropicMessages && is_official_anthropic(&base_url) => {
            env_first(&["ANTHROPIC_API_KEY", "ARA_API_KEY", "ARA_TEST_API_KEY"]).map(|(_, v)| v)
        }
        None if args.api() == Api::AnthropicMessages => {
            env_first(&["ARA_API_KEY", "ARA_TEST_API_KEY"]).map(|(_, v)| v)
        }
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
        let (k, v) = h
            .split_once(':')
            .with_context(|| format!("--header {h:?} must be `Name: value`"))?;
        extra_headers.push((k.trim().to_string(), v.trim().to_string()));
    }
    let mut stream_options = StreamOptions {
        api_key,
        extra_headers,
        no_strict: ara_cli::schema_policy::process_schema_policy().no_strict(),
        ..Default::default()
    };
    stream_options
        .compat
        .requires_reasoning_content_on_all_assistant_turns = args.chat_replay_reasoning_content;
    if args.chat_replay_reasoning_content {
        stream_options
            .compat
            .requires_reasoning_content_for_tool_calls = true;
        stream_options
            .compat
            .allows_synthetic_reasoning_content_for_tool_calls = false;
    }
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
        startup: None,
    })
}

/// Classify the original explicit wire contract without treating a failed
/// Native selection as a reason to bypass roles, scopes or thinking grammar.
fn is_unscoped_explicit_raw_model(args: &Args) -> Result<bool> {
    if !args.models.is_empty() {
        return Ok(false);
    }
    let model = args
        .model
        .clone()
        .or_else(|| env_first(&["ARA_MODEL", "ARA_TEST_MODEL_ID"]).map(|(_, value)| value));
    let Some(model) = model else { return Ok(false) };
    if model.is_empty() || args.api() == Api::OpenaiCodexResponses {
        return Ok(false);
    }
    let pattern = ara_prompt::js::trim(&model);
    if pattern.is_empty()
        || pattern.starts_with('@')
        || pattern.starts_with("pi/")
        || pattern.contains(['*', '?', '[', ','])
        || pattern.rsplit_once(':').is_some_and(|(_, suffix)| {
            ara_cli::session_thinking::parse_configured(suffix).is_some()
        })
    {
        return Ok(false);
    }
    let agent_dir = ara_home().join("agent");
    if !rpc_host_settings::enabled_model_patterns(&agent_dir)?.is_empty()
        || rpc_host_settings::AutoThinkingSettings::load(&agent_dir)?
            .roles
            .contains_key(pattern)
    {
        return Ok(false);
    }
    Ok(true)
}

/// Published daily wire routes may have a provider transport override without
/// a catalog model declaration. Admit that authored route before Native model
/// selection; a Native selection error must never become a raw-route fallback.
fn is_explicit_raw_startup_route(
    args: &Args,
    config: Option<&ara_cli::models_config::ModelsConfig>,
    registry: &ara_cli::model_registry::ModelRegistry,
) -> Result<bool> {
    if !is_unscoped_explicit_raw_model(args)? {
        return Ok(false);
    }
    let model = args
        .model
        .clone()
        .or_else(|| env_first(&["ARA_MODEL", "ARA_TEST_MODEL_ID"]).map(|(_, value)| value));
    let provider = args
        .provider
        .clone()
        .or_else(|| env_first(&["ARA_PROVIDER"]).map(|(_, value)| value));
    let (Some(model), Some(provider)) = (model, provider) else {
        return Ok(false);
    };
    if provider == "openai-codex" {
        return Ok(false);
    }
    let authored_provider =
        config.and_then(|config| config.value().get("providers")?.get(&provider));
    if authored_provider.is_some_and(|value| {
        value
            .get("models")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|models| !models.is_empty())
            || value.get("discovery").is_some()
    }) {
        return Ok(false);
    }
    if registry
        .find_reference(&provider.as_str().into(), &model.as_str().into())?
        .is_some()
    {
        return Ok(false);
    }
    let has_authored_endpoint = args
        .base_url
        .as_deref()
        .is_some_and(|value| !value.is_empty())
        || env_first(&["ARA_BASE_URL", "ARA_TEST_BASE_URL", "OPENROUTER_BASE_URL"]).is_some()
        || authored_provider
            .and_then(|value| value.get("baseUrl"))
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| !value.is_empty());
    if !has_authored_endpoint {
        return Ok(false);
    }
    // Keep the original explicit provider and wire ID, including slash-qualified
    // IDs and literal non-thinking colon suffixes, for the daily projection.
    Ok(true)
}

async fn resolve_startup_route(
    args: &mut Args,
    project_dir: &Path,
    account_auth: &mut Option<Arc<ara_cli::auth_storage::AuthStorage>>,
    registry_cancel: &CancellationToken,
    resumed_journal: Option<&SessionJournal>,
) -> Result<Route> {
    Box::pin(resolve_startup_route_scoped(args, project_dir, account_auth, registry_cancel, resumed_journal, None, None, false)).await
}

async fn resolve_startup_route_scoped(
    args: &mut Args,
    project_dir: &Path,
    account_auth: &mut Option<Arc<ara_cli::auth_storage::AuthStorage>>,
    registry_cancel: &CancellationToken,
    resumed_journal: Option<&SessionJournal>,
    agent_context: Option<Arc<rpc_host::HostAgentContext>>,
    shared_registry: Option<ara_cli::model_registry::ModelRegistry>,
    force_catalog: bool,
) -> Result<Route> {
    use ara_cli::daily_model_config::{
        DailyOverrides, load_daily_config, resolve_daily_selection_with_reference,
        resolve_daily_selection_with_registry, validate_daily_auth_ownership,
    };
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
    let discovery_host = ara_cli::auth_broker_discover::BrokerDiscoveryHost::for_process(
        ara_home(),
        Some(key_resolver.clone()),
    );
    let broker_config = if agent_context.is_some() && account_auth.is_some() { None } else {
        ara_cli::auth_broker_discover::BrokerConfigResolver::new(discovery_host.clone())
            .resolve(registry_cancel).await?
    };
    if !force_catalog && broker_config.is_none()
        && args.models_config.is_none()
        && matches!(args.api(), Api::AnthropicMessages | Api::ProxyAuto)
    {
        let route = resolve_route(args)?;
        if !agent_context.as_ref().is_some_and(|context| context.auth_scope.oauth_account_pools.contains_key(&route.model.provider)) {
            return Ok(route);
        }
    }
    let path = args
        .models_config
        .clone()
        .unwrap_or_else(|| ara_home().join("agent/models.yml"));
    let config = load_daily_config(&path, args.models_config.is_some())?;
    let configured_provider = args
        .provider
        .as_deref()
        .map(str::to_owned)
        .or_else(|| std::env::var("ARA_PROVIDER").ok());
    if !force_catalog && config.is_none()
        && broker_config.is_none()
        && args.api() != Api::OpenaiCodexResponses
        && configured_provider.as_deref() != Some("openai-codex")
        && !ara_home().join("agent/auth.db").exists()
        && is_unscoped_explicit_raw_model(args)?
    {
        let route = resolve_route(args)?;
        if !agent_context.as_ref().is_some_and(|context| context.auth_scope.oauth_account_pools.contains_key(&route.model.provider)) {
            return Ok(route);
        }
    }
    let headers = args
        .headers
        .iter()
        .map(|header| {
            let (name, value) = header
                .split_once(':')
                .context("--header must be Name: value")?;
            Ok((name.trim().to_owned(), value.trim().to_owned()))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut overrides = DailyOverrides {
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
    validate_daily_auth_ownership(
        config.as_ref(),
        &overrides,
        &ara_cli::schema_policy::process_daily_environment,
    )?;
    // Ordinary routes without stored accounts use environment credentials and
    // do not create an unrelated account database during catalog startup.
    if agent_context.is_some() && let Some(account) = account_auth.as_ref() {
        host.credentials = Arc::new(ara_cli::auth_storage_registry::AuthStorageRegistryCredentials::new(
            account.as_ref().clone(), registry_cancel.clone(),
        ));
    } else if let Some(config) = broker_config {
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
        host.credentials = Arc::new(
            ara_cli::auth_storage_registry::AuthStorageRegistryCredentials::new(
                account.as_ref().clone(),
                registry_cancel.clone(),
            ),
        );
    } else if configured_provider.as_deref() == Some("openai-codex")
        || ara_home().join("agent/auth.db").exists()
    {
        let codex = open_codex_auth(
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
        )
        .await?;
        #[cfg(feature = "test-fixture")]
        let reset_credit_client = std::env::var("ARA_TEST_CODEX_AUTH_BASE_URL")
            .ok()
            .map(|base| {
                ara_cli::codex_usage::CodexUsageProvider::with_fixture_endpoint_resolver(Arc::new(
                    move |canonical| {
                        let path = reqwest::Url::parse(canonical)
                            .map(|url| url.path().to_owned())
                            .unwrap_or_default();
                        format!("{}{path}", base.trim_end_matches('/'))
                    },
                ))
            })
            .transpose()?;
        let account = Arc::new(ara_cli::auth_storage::AuthStorage::for_codex(
            codex,
            ara_cli::auth_storage::AuthStorageOptions {
                config_key_resolver: Arc::new(
                    ara_cli::config_request_auth::ConfigStorageKeyResolver::new(
                        project_dir.to_path_buf(),
                        host.config_values.clone(),
                        host.config_environment.clone(),
                    ),
                ),
                environment: Arc::new(ara_cli::auth_storage_registry::CatalogAuthEnvironment(
                    host.factory.environment.clone(),
                )),
                #[cfg(feature = "test-fixture")]
                reset_credit_client,
                ..Default::default()
            },
        )?);
        #[cfg(feature = "test-fixture")]
        if let Ok(base) = std::env::var("ARA_TEST_CODEX_AUTH_BASE_URL") {
            let usage = ara_cli::codex_usage::CodexUsageProvider::with_fixture_endpoint_resolver(
                Arc::new(move |canonical| {
                    let path = reqwest::Url::parse(canonical)
                        .map(|url| url.path().to_owned())
                        .unwrap_or_default();
                    format!("{}{path}", base.trim_end_matches('/'))
                }),
            )?;
            account.register_usage_provider("openai-codex", Arc::new(usage))?;
        }
        // Install the owner before any Registry preflight can refresh a grant,
        // including failures before the request factory is built.
        *account_auth = Some(account.clone());
        host.credentials = Arc::new(
            ara_cli::auth_storage_registry::AuthStorageRegistryCredentials::new(
                account.as_ref().clone(),
                registry_cancel.clone(),
            ),
        );
    }
    let mut registry = if let Some(registry) = shared_registry { registry } else { ara_cli::model_registry::ModelRegistry::open(
        ara_cli::model_config_file::ModelsConfigFile::new(&path)?,
        Default::default(),
        host,
    )
    .await? };
    if registry.config_error().is_some() {
        bail!("models configuration changed or could not be loaded; correct the file");
    }
    if let Some((context, account)) = agent_context.as_ref().zip(account_auth.as_ref()) {
        registry = registry.with_session_scope(account.as_ref().clone(), context.auth_scope.clone());
    }
    if let Some(provider) = configured_provider.as_ref() {
        let mut selected = ara_cli::model_patch::OrderedProviderSet::default();
        selected.insert(provider.as_str().into());
        registry
            .runtime()
            .refresh_discoverable_providers(
                selected,
                ara_cli::model_manager::ModelRefreshStrategy::OnlineIfUncached,
            )
            .await?;
    }
    let config = registry.config();
    let (selection, startup) = if !force_catalog && is_explicit_raw_startup_route(args, config.as_ref(), &registry)? {
        // The original projection retains the installed Broker/shared-storage
        // auth owner and validates its actual raw API/endpoint/header contract.
        (
            resolve_daily_selection_with_registry(
                config.as_ref(),
                &overrides,
                &ara_cli::schema_policy::process_daily_environment,
                &registry,
            )?,
            None,
        )
    } else {
        let startup =
            startup_selection::resolve(args, &registry, resumed_journal, registry_cancel).await?;
        let target = startup.model;
        overrides.provider = Some(startup_selection::model_text(&target, "provider")?);
        overrides.model = Some(startup_selection::model_text(&target, "id")?);
        let config = registry.config();
        validate_daily_auth_ownership(
            config.as_ref(),
            &overrides,
            &ara_cli::schema_policy::process_daily_environment,
        )?;
        (
            resolve_daily_selection_with_reference(
                config.as_ref(),
                &overrides,
                &ara_cli::schema_policy::process_daily_environment,
                &target,
            )?,
            Some(startup.state),
        )
    };
    args.api = Some(Api::from_str(selection.api.as_str(), false).map_err(anyhow::Error::msg)?);
    // This CLI switch controls Responses reasoning items. A configured Chat
    // model's reasoning capability is carried by DailySelection/its policy;
    // it must not be turned into the Responses-only command-line switch.
    args.reasoning |= selection.model.reasoning
        && matches!(args.api(), Api::OpenaiResponses | Api::OpenaiCodexResponses);
    args.max_tokens = selection.generation.max_tokens;
    args.temperature = selection.generation.temperature;
    validate_route_args(args)?;
    Ok(Route {
        model: selection.model.clone(),
        stream_options: StreamOptions::default(),
        daily: Some(selection),
        registry: Some(registry),
        startup,
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
    context: &mut Vec<AgentMessage>,
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
    context: &mut Vec<AgentMessage>,
    sink: &HostSink,
    cancel: &CancellationToken,
    manual: bool,
    selection: Option<&ara_cli::snapcompact::ManualCompactArgs>,
) -> Result<bool> {
    let tokens = sink.projector.count_current(
        context,
        &config.model,
        ara_agent::tokenizer::MessageCountOptions::default(),
    )?;
    if !manual && (args.compact_threshold == 0 || tokens <= args.compact_threshold) {
        return Ok(false);
    }
    let policy = rpc_host_settings::AutoCompactionPolicy::load(&ara_home().join("agent"))?;
    if !manual && !policy.enabled() {
        return Ok(false);
    }
    let settings = policy.recovery_settings()?;
    let methods = selection
        .and_then(|selection| selection.methods.as_ref())
        .unwrap_or(&settings.method_order);
    let focus = selection.and_then(|selection| selection.focus.as_deref());
    for method in methods {
        if cancel.is_cancelled() {
            return Err(ara_cli::handoff::cancelled());
        }
        let result = match method.as_str() {
            "remote" => {
                run_remote_compaction(
                    args, config, factory, &settings, context, sink, cancel, manual, focus,
                )
                .await
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
                        selection.methods.as_ref().is_some_and(|methods| {
                            methods.len() == 1 && methods[0] == "snapcompact"
                        })
                    }),
                )
                .await
            }
            "soft" => {
                run_soft_compaction(args, config, factory, context, sink, cancel, manual, focus)
                    .await
            }
            // These automatic REPL methods retain their separately recorded
            // implementation scope; /handoff and /shake remain explicit commands.
            _ => continue,
        };
        match result {
            Ok(true) => return Ok(true),
            Ok(false) => {}
            Err(error)
                if ara_cli::remote_compaction::is_cancelled(&error)
                    || sink.persist_failed.load(Ordering::SeqCst) =>
            {
                return Err(error);
            }
            Err(error)
                if selection.is_some_and(|selection| {
                    selection
                        .methods
                        .as_ref()
                        .is_some_and(|methods| methods.len() == 1)
                }) =>
            {
                return Err(error);
            }
            Err(error) => eprintln!(
                "ara: {method} compaction failed ({error:#}); trying the next configured method"
            ),
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
    context: &mut Vec<AgentMessage>,
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
        deployment: Some(ara_cli::snapcompact::deployment_target(
            &config.model,
            factory.metadata.as_ref(),
        )?),
    };
    let source = snapshot.clone();
    let model = config.model.clone();
    let keep_tokens = args.compact_keep_tokens;
    let tokens_before = sink
        .projector
        .count_current(context, &config.model, Default::default())? as u64;
    let prepared = tokio::task::spawn_blocking(move || {
        ara_cli::snapcompact::prepare_snapcompact(
            &source,
            &model,
            keep_tokens,
            tokens_before,
            &policy,
        )
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
    let journal = guard
        .as_mut()
        .context("Session unavailable during snapcompact")?;
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
            let _ = factory.adopt_context(journal, context, sink);
        }
        return Err(error.into());
    }
    let selected = ara_cli::remote_compaction::route_raw_context(
        journal,
        &sink.projector,
        &config.model,
        factory.metadata.as_ref(),
        &settings.remote,
        factory.route.remote_supports_images(),
    )?;
    *context = sink.projector.reconcile_owned_context(context, selected)?;
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
    context: &mut Vec<AgentMessage>,
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
        let Some(journal) = guard.as_ref().filter(|journal| journal.is_persistent()) else {
            return Ok(false);
        };
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
            oneshot_retry: if manual {
                SummaryOptions::default().oneshot_retry
            } else {
                None
            },
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
            if let Some(failure) =
                error.downcast_ref::<ara_cli::remote_compaction::RemotePreparationError>()
            {
                record_remote_attempts(sink, &artifacts, &failure.attempts).await?;
            }
            return Err(error);
        }
    };
    let tokens_before = sink.projector.count_current(
        context,
        &config.model,
        ara_agent::tokenizer::MessageCountOptions::default(),
    )? as u64;
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
    eprintln!(
        "ara: remote compaction saved in the current Session; {} request attempts",
        prepared.attempts.len()
    );
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
    let content =
        serde_json::to_string(&serde_json::json!({"method":"remote", "attempts":receipts}))?;
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
    context: &mut Vec<AgentMessage>,
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
    let tokens = sink
        .projector
        .count_current(context, model, MessageCountOptions::default())?;
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
    let previous_summary = snapshot
        .previous_summary
        .as_ref()
        .map(|summary| summary.previous_summary_for_text_compaction());
    let previous_summary = previous_summary.as_deref();
    let cut = match select_native_entry_compaction_cut(
        &sources,
        args.compact_keep_tokens,
        previous_summary,
    ) {
        Ok(Some(cut)) => cut,
        Ok(None) => {
            notice("no earlier message prefix can be summarized at this retention target".into());
            return Ok(false);
        }
        Err(error) => {
            notice(format!(
                "no earlier message prefix can be summarized ({error})"
            ));
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
    let summary_provider: &dyn ModelProvider = if model.api == "openai-codex-responses" {
        &uncapped
    } else {
        provider.as_ref()
    };
    let options = SummaryOptions {
        oneshot_retry: if manual {
            SummaryOptions::default().oneshot_retry
        } else {
            None
        },
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
                eprintln!(
                    "ara: Codex summary exceeds the local adoption budget; session untouched"
                );
                return Ok(false);
            }
            // Show the stop reason, output tokens and the provider's status and
            // message so the cause is visible.
            let stop = e
                .stop_reason
                .map(|r| format!(", stop reason {}", r.as_str()))
                .unwrap_or_default();
            let output = e
                .usage
                .as_ref()
                .and_then(|u| u.output)
                .map(|n| format!(", {n} output tokens"))
                .unwrap_or_default();
            let status = e
                .provider_status
                .map(|s| format!(", HTTP {s}"))
                .unwrap_or_default();
            let detail = e
                .provider_message
                .as_deref()
                .map(|m| format!(": {}", sanitize_text(m)))
                .unwrap_or_default();
            eprintln!(
                "ara: summary call failed ({e}{stop}{output}{status}{detail}); session untouched"
            );
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
    let kept = sink
        .projector
        .count_current(context, model, MessageCountOptions::default())?;
    eprintln!(
        "ara: compacted {tokens} estimated tokens down to {kept}; summary persisted with source IDs"
    );
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
        let mut sigint =
            signal(SignalKind::interrupt()).context("installing the SIGINT handler")?;
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
async fn interruptible<F: Future>(
    step: F,
    token: &CancellationToken,
    interrupts: &mut Interrupts,
) -> F::Output {
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
    let Some(storage) = factory
        .shared_account
        .as_ref()
        .or(factory.account_auth.as_ref())
    else {
        eprintln!("ara: provider quota reporting is unavailable for this route");
        return Ok(());
    };
    if refresh {
        storage
            .invalidate_usage_cache_and_notify(None, cancel)
            .await?;
    }
    let selected = factory.route.model().clone();
    let registry = factory.registry.clone();
    let selected_provider = selected.provider.clone();
    let selected_url = selected.base_url.clone();
    let options = FetchUsageReportsOptions {
        context: AuthRequestContext {
            session_id: session_id.clone(),
            model_id: Some(selected.id.clone()),
            ..Default::default()
        },
        base_url_resolver: Some(Arc::new(move |provider| {
            if provider == selected_provider {
                Some(selected_url.clone())
            } else {
                registry
                    .as_ref()?
                    .get_provider_base_url(&provider.into())
                    .ok()??
                    .to_utf8()
                    .ok()
            }
        })),
        ..Default::default()
    };
    let reports = if let Some(controller) = &factory.reset_controller {
        let mut context = factory.reset_context(&selected, session_id);
        context.usage = options;
        controller.fetch_usage_reports(&context, cancel).await?
    } else {
        storage
            .fetch_usage_reports_with_options(&options, cancel)
            .await?
    };
    let Some(reports) = reports else {
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
                ["email", "accountId", "projectId"]
                    .into_iter()
                    .find_map(|field| {
                        metadata
                            .get(field)?
                            .as_str()
                            .filter(|value| !value.is_empty())
                            .map(str::to_owned)
                    })
            })
            .unwrap_or_else(|| "account".into());
        eprintln!(
            "  {} — {}",
            sanitize_text(&report.provider),
            sanitize_text(&account)
        );
        if report.limits.is_empty() {
            eprintln!("    no limits reported");
        }
        for limit in report.limits {
            let amount = &limit.amount;
            let used = amount
                .used
                .or_else(|| amount.used_fraction.map(|fraction| fraction * 100.0));
            let unit = if amount.unit == UsageUnit::Percent {
                "%".to_owned()
            } else {
                format!(
                    " {}",
                    serde_json::to_value(amount.unit)?
                        .as_str()
                        .unwrap_or("unknown")
                )
            };
            let used = used
                .map(|used| format!("{used:.2}{unit} used"))
                .unwrap_or_else(|| "unknown used".into());
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

async fn run_repl_usage_reset(
    factory: &ProviderFactory,
    session_id: Option<String>,
    selector: &str,
    cancel: &CancellationToken,
) -> Result<()> {
    use ara_cli::codex_reset_controller::label;
    let Some(controller) = &factory.reset_controller else {
        eprintln!("ara: saved Codex resets are unavailable for this route");
        return Ok(());
    };
    let context = factory.reset_context(factory.route.model(), session_id);
    if selector.is_empty() {
        let statuses = controller.list(&context, cancel).await?;
        if statuses.is_empty() {
            eprintln!("ara: no stored Codex accounts available");
        }
        for status in statuses {
            let detail = if status.error.is_some() {
                "unreachable; remaining resets unknown".to_owned()
            } else {
                format!("{} saved resets", status.available_count)
            };
            eprintln!(
                "  {}{}: {}",
                sanitize_text(label(&status)),
                if status.active { " (active)" } else { "" },
                detail
            );
        }
        return Ok(());
    }
    let Some(result) = controller.manual(context, selector, cancel).await? else {
        eprintln!(
            "ara: no stored Codex account matches {}",
            sanitize_text(selector)
        );
        return Ok(());
    };
    let account = result
        .outcome
        .email
        .as_deref()
        .or(result.outcome.account_id.as_deref())
        .unwrap_or("account");
    let message = match result.outcome.code.as_str() {
        "reset" if result.outcome.ok => "saved reset redeemed",
        "already_redeemed" => "saved reset was already redeemed elsewhere",
        "no_credit" => "no saved resets available",
        "nothing_to_reset" => "nothing to reset",
        "no_account" => "account is unavailable",
        "account_unavailable" | "credit_list_failed" => "account or live reset list is unreachable",
        "outcome_unknown" => "reset outcome unknown; account fenced, consume will not be replayed",
        _ => "saved reset request was declined",
    };
    eprintln!("ara: {}: {message}", sanitize_text(account));
    if result.outcome.settlement_error.is_some() {
        if result.outcome.ok && result.outcome.code == "reset" {
            eprintln!(
                "ara: upstream reset is confirmed, but receipt or local quota settlement failed; automatic replay remains blocked"
            );
        } else {
            eprintln!(
                "ara: receipt or local quota settlement failed; no automatic replay is authorized"
            );
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
    tool_owner: CliToolOwner,
    native_jobs: native_setup::NativeSessionJobs,
    provider_factory: &'a ProviderFactory,
    mcp_config: &'a Option<McpServerConfig>,
    artifact_router: Arc<ara_cli::session_artifacts::ArtifactUriRouter>,
}

#[allow(clippy::too_many_arguments)]
async fn run_local_shake(
    mode: &str,
    model: &ara_ai::Model,
    factory: &ProviderFactory,
    context: &mut Vec<AgentMessage>,
    sink: &HostSink,
    artifacts: &ara_cli::session_artifacts::SessionArtifacts,
    non_message: usize,
    cancel: &CancellationToken,
) -> Result<bool> {
    use ara_agent::compaction::local_reduction::ShakeConfig;
    use ara_cli::local_reduction::{
        HostReductionPolicy, prepare_images_after_boundary, prepare_shake,
        prepare_thinking_after_boundary,
    };
    let settings = rpc_host_settings::AutoCompactionPolicy::load(&ara_home().join("agent"))?
        .recovery_settings()?;
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
            prepare_shake(
                snapshot,
                model,
                &ShakeConfig::aggressive(),
                &policy,
                artifacts,
                ara_ai::now_ms(),
            )
            .await?
        }
        "images" => prepare_images_after_boundary(snapshot, model, native_boundary.as_deref())?,
        "thinking" => prepare_thinking_after_boundary(snapshot, model, native_boundary.as_deref())?,
        _ => bail!("Unknown shake mode: {mode}. Expected elide, images or thinking"),
    };
    if cancel.is_cancelled() {
        bail!("Shake aborted");
    }
    if mode == "elide"
        && let Some(edit) = ara_cli::context_budget::anchor_edit(
            &prepared.snapshot,
            &prepared.entry_tokens_freed,
            non_message,
        )?
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
        prepared
            .artifact_id
            .map(|id| format!("; recover: artifact://{id}"))
            .unwrap_or_default()
    );
    Ok(true)
}

async fn run_repl_handoff(
    config: &AgentConfig,
    factory: &ProviderFactory,
    keep_tokens: usize,
    focus: Option<&str>,
    context: &mut Vec<AgentMessage>,
    sink: &HostSink,
    cancel: &CancellationToken,
) -> Result<bool> {
    let snapshot = sink
        .journal
        .lock()
        .await
        .as_ref()
        .context("Session unavailable")?
        .native_handoff_snapshot()?;
    let side = factory
        .route
        .bind_side_request(factory.client.clone(), &snapshot.projection.session_id);
    let deadline = config
        .deadline
        .unwrap_or_else(|| Instant::now() + Duration::from_secs(120));
    let prepared = if sink.projector.actual_values().is_some() {
        ara_cli::handoff::prepare_actual_handoff(
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
        .await
    } else {
        ara_cli::handoff::prepare_handoff(
            &snapshot,
            &sink.projector.estimate_current(context, &config.model)?,
            config,
            side.as_ref(),
            keep_tokens,
            focus,
            false,
            deadline,
            cancel,
        )
        .await
    }?;
    let Some(prepared) = prepared else {
        return Ok(false);
    };
    if cancel.is_cancelled() {
        bail!("Handoff cancelled");
    }
    let tokens_before = sink.projector.count_current(
        context,
        &config.model,
        ara_agent::tokenizer::MessageCountOptions::default(),
    )? as u64;
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
// Allocate the complete REPL dispatcher outside its caller's async poll frame.
// The original Session, interrupt and settlement owners remain in the body.
#[allow(clippy::too_many_arguments)]
fn run_repl_loop<'a>(
    args: &'a Args,
    model: &'a ara_ai::Model,
    provider: &'a Arc<dyn ModelProvider>,
    system_prompt: &'a [String],
    tools: &'a [Arc<dyn ara_agent::AgentTool>],
    hooks: &'a Arc<dyn LoopHooks>,
    context: &'a mut Vec<AgentMessage>,
    sink: &'a Arc<HostSink>,
    cancel: &'a CancellationToken,
    session: ReplSession<'a>,
    thinking: host_thinking::CliThinking,
    interrupts: Interrupts,
) -> Pin<Box<impl Future<Output = Result<i32>> + 'a>> {
    Box::pin(run_repl_loop_owned(
        args,
        model,
        provider,
        system_prompt,
        tools,
        hooks,
        context,
        sink,
        cancel,
        session,
        thinking,
        interrupts,
    ))
}

#[allow(clippy::too_many_arguments)]
async fn run_repl_loop_owned(
    args: &Args,
    model: &ara_ai::Model,
    provider: &Arc<dyn ModelProvider>,
    system_prompt: &[String],
    tools: &[Arc<dyn ara_agent::AgentTool>],
    hooks: &Arc<dyn LoopHooks>,
    context: &mut Vec<AgentMessage>,
    sink: &Arc<HostSink>,
    cancel: &CancellationToken,
    mut session: ReplSession<'_>,
    mut thinking: host_thinking::CliThinking,
    mut interrupts: Interrupts,
) -> Result<i32> {
    let mut sink_owner = sink.clone();
    let mut provider = provider.clone();
    let mut system_prompt = system_prompt.to_vec();
    let mut hooks = hooks.clone();
    let mut tools = tools.to_vec();
    let mut tool_owner = session.tool_owner.clone();
    eprintln!(
        "ara: interactive session ({}). /help for commands.",
        model.id
    );
    if let Some(path) = sink_owner
        .journal
        .lock()
        .await
        .as_ref()
        .filter(|j| j.is_persistent())
        .map(|j| j.path().to_path_buf())
        && path.exists()
    {
        eprintln!("ara: session {}", path.display());
    }
    let mut turn: usize = 0;
    let mut pending_read = None;
    let code = loop {
        // Settled callbacks retain their original sink; new Sessions get a new Arc.
        let sink = &sink_owner;
        sink.bind_service_session().await?;
        if let Some(host)=sink.task_host(){host.apply_idle_context(context);}
        if let Err(error) = sink
            .refresh_terminal_history(
                tool_owner
                    .native
                    .as_ref()
                    .map(|owner| owner.prepared.clone()),
            )
            .await
        {
            if let Some(terminal) = &sink.terminal {
                terminal.diagnostic(&format!("ara: Session display refresh failed ({error})"));
            }
        }
        let id = sink
            .journal
            .lock()
            .await
            .as_ref()
            .map(|journal| journal.session_id().to_owned());
        session.provider_factory.refresh_usage_if_stale(model, id);
        if tool_owner.edit_session.as_ref().is_some_and(|owner| owner.has_pending())
            || sink.service_session.as_ref().is_some_and(|owner| owner.has_pending())
            || sink.task_host().is_some_and(|host|host.has_idle_input())
        {
            let prepared = tool_owner.native.as_ref().map(|owner| owner.prepared.clone());
            let task_input=sink.task_host().map(|host|host.idle_input()).transpose()?.flatten();
            let message = if task_input.is_some(){task_input}else if let Some(service) = &sink.service_session
                && service.has_pending()
            {
                service.idle_message(sink.projector.clone(), prepared)?
            } else {
                tool_owner.edit_session.as_ref().expect("checked diagnostic owner")
                    .idle_message(sink.projector.clone(), prepared)?
            };
            if let Some(message) = message {
                let token = cancel.child_token();
                let config = AgentConfig {
                    model: model.clone(),
                    provider: thinking.bind(provider.clone()),
                    system_prompt: system_prompt.clone(),
                    tools: tools.clone(),
                    tool_choice: None,
                    max_tokens: args.max_tokens,
                    temperature: args.temperature,
                    deadline: args
                        .max_time
                        .map(|seconds| Instant::now() + Duration::from_secs_f64(seconds.max(0.0))),
                    max_model_calls: args.max_model_calls,
                    hooks: hooks.clone(),
                    interrupt_mode: rpc_host_settings::load_interrupt_mode(
                        &ara_home().join("agent"),
                    )?,
                };
                host_edit_post_apply::install_cli(
                    args,
                    session.provider_factory,
                    session.cwd,
                    &tool_owner,
                    &config.model,
                    &config.hooks,
                    sink,
                )
                .await;
                let guard =
                    make_streaming_edit_guard(&tool_owner, Some(&ara_home().join("agent")))?;
                let step = run_host_agent(
                    vec![message],
                    context,
                    &config,
                    &token,
                    sink.as_ref(),
                    tool_owner.native.as_ref(),
                    guard,
                );
                let report = interruptible(step, &token, &mut interrupts).await?;
                sink.end_stream_line();
                if sink.persist_failed.load(Ordering::SeqCst) {
                    break 1;
                }
                if !matches!(report.end, RunEnd::Completed | RunEnd::TerminalToolResult) {
                    eprintln!(
                        "ara: late diagnostics continuation ended {:?}; Session kept",
                        report.end
                    );
                }
            }
            continue;
        }
        let prompt_terminal = sink.terminal.clone();
        if prompt_terminal.is_none() {
            eprint!("> ");
            let _ = std::io::stderr().flush();
        }
        let prompt_ticket = Arc::new(ara_cli::terminal::driver::PromptReadTicket::default());
        let reader_ticket = prompt_ticket.clone();
        let mut read = pending_read.take().unwrap_or_else(|| {
            tokio::task::spawn_blocking(move || {
                if let Some(terminal) = prompt_terminal {
                    return terminal.read_prompt_with_ticket(&reader_ticket);
                }
                let mut buf = String::new();
                match std::io::stdin().read_line(&mut buf) {
                    Ok(0) => Ok(None),
                    Ok(_) => Ok(Some(buf)),
                    Err(e) => Err(e.to_string()),
                }
            })
        });
        let edit_wake = tool_owner.edit_session.as_ref().map(|owner| owner.wake());
        let service_wake = sink.service_session.as_ref().map(|owner| owner.wake());
        let task_wake=sink.task_host().map(|host|host.wake.clone());
        let mut suspended_for_late = false;
        let display_port = sink.terminal.as_ref().map(|t| t.display_port());
        let display_wake = if let Some(display) = &sink.display {
            Some(display.lock().await.wake())
        } else {
            None
        };
        let line = loop {
            // The same Session evaluator runs while the existing blocking
            // reader owns input. No OS thread executes retained callbacks.
            let notice = async {
                if let Some(wake) = &display_wake {
                    wake.notified().await
                } else {
                    std::future::pending::<()>().await
                }
            };
            let demand = async {
                if let Some(port) = &display_port {
                    port.ready().await
                } else {
                    std::future::pending::<()>().await
                }
            };
            let more_display = if let Some(display) = &sink.display {
                display.lock().await.pump_quantum()
            } else {
                false
            };
            tokio::select! {
                _=tokio::task::yield_now(),if more_display=>{},
                line=&mut read=>break line.unwrap_or_else(|e|Err(e.to_string())),
                Some(())=interrupts.recv()=>{
                    if let Some(terminal)=&sink.terminal{terminal.cancel_prompt();let _=(&mut read).await;}
                    break Err("terminal prompt interrupted".into());
                },
                _=async { tokio::select!{
                    _=async{if let Some(wake)=&edit_wake{wake.notified().await}else{std::future::pending::<()>().await}}=>{},
                    _=async{if let Some(wake)=&service_wake{wake.notified().await}else{std::future::pending::<()>().await}}=>{},
                    _=async{if let Some(wake)=&task_wake{
                        let notice=wake.notified();tokio::pin!(notice);notice.as_mut().enable();
                        if !sink.task_host().is_some_and(|host|host.has_idle_input()||host.has_context_updates()){notice.await;}
                    }else{std::future::pending::<()>().await}}=>{},
                }}=>{
                    if tool_owner.edit_session.as_ref().is_some_and(|owner| owner.has_pending())
                        || sink.service_session.as_ref().is_some_and(|owner| owner.has_pending())
                        || sink.task_host().is_some_and(|host|host.has_idle_input()||host.has_context_updates()) {
                        if let Some(terminal)=&sink.terminal {
                            terminal.suspend_prompt(&prompt_ticket);
                            let joined=(&mut read).await.unwrap_or_else(|error|Err(error.to_string()));
                            match joined {
                                Err(error) if error==ara_cli::terminal::driver::PASSIVE_PROMPT_SUSPENDED => {
                                    suspended_for_late=true;
                                    break Err(error);
                                }
                                // Enter/EOF/error won the race: admit/exit through
                                // the original path; late aside stays queued.
                                result => break result,
                            }
                        } else {
                            suspended_for_late=true;
                            break Err("late diagnostics idle wake".into());
                        }
                    }
                },
                _=notice=>{},_=demand=>{},
            }
        };
        if suspended_for_late {
            if sink.terminal.is_none() {
                pending_read = Some(read);
            }
            continue;
        }
        let raw = match line {
            Err(error) if error == "terminal prompt interrupted" => {
                eprintln!();
                break 130;
            }
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
                    "commands: /help, /new, /clear, /compact, /exit, /usage [refresh|reset [active|email|account]], /shake [elide|images|thinking], /handoff [focus], /skill:<name> [arguments]"
                );
                for skill in session.skills {
                    eprintln!(
                        "  /skill:{} — {}",
                        sanitize_text(&skill.name),
                        sanitize_text(&skill.description)
                    );
                }
                eprintln!("Ctrl+C during a turn cancels it; Ctrl+C at the prompt exits.");
                continue;
            }
            "/usage" | "/usage refresh" => {
                let token = cancel.child_token();
                let id = sink
                    .journal
                    .lock()
                    .await
                    .as_ref()
                    .map(|journal| journal.session_id().to_owned());
                let step = run_repl_usage(
                    session.provider_factory,
                    id,
                    input == "/usage refresh",
                    &token,
                );
                if let Err(error) = interruptible(step, &token, &mut interrupts).await {
                    eprintln!("ara: usage query failed ({error:#})");
                }
                continue;
            }
            command if command == "/usage reset" || command.starts_with("/usage reset ") => {
                let token = cancel.child_token();
                let id = sink
                    .journal
                    .lock()
                    .await
                    .as_ref()
                    .map(|journal| journal.session_id().to_owned());
                let selector = ara_prompt::js::trim(command.trim_start_matches("/usage reset"));
                let step = run_repl_usage_reset(session.provider_factory, id, selector, &token);
                if let Err(error) = interruptible(step, &token, &mut interrupts).await {
                    eprintln!(
                        "ara: saved reset query failed ({error:#}); no confirmed reset receipt returned"
                    );
                }
                continue;
            }
            command if command == "/compact" || command.starts_with("/compact ") => {
                let selection = match ara_cli::snapcompact::parse_manual_args(
                    command.trim_start_matches("/compact"),
                ) {
                    Ok(selection) => selection,
                    Err(error) => {
                        eprintln!("ara: {error}");
                        continue;
                    }
                };
                let token = cancel.child_token();
                let config = AgentConfig {
                    model: model.clone(),
                    provider: thinking.bind(provider.clone()),
                    system_prompt: system_prompt.clone(),
                    tools: tools.to_vec(),
                    tool_choice: None,
                    max_tokens: args.max_tokens,
                    temperature: args.temperature,
                    deadline: None,
                    max_model_calls: args.max_model_calls,
                    hooks: hooks.clone(),
                    interrupt_mode: rpc_host_settings::load_interrupt_mode(
                        &ara_home().join("agent"),
                    )?,
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
                        let id = sink
                            .journal
                            .lock()
                            .await
                            .as_ref()
                            .context("Session unavailable")?
                            .session_id()
                            .to_owned();
                        provider = session.provider_factory.bind_codex_session(id, None);
                    }
                    Ok(false) => {}
                    Err(e) => {
                        eprintln!("ara: compaction failed ({e:#}); inspect the Session receipt")
                    }
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
                let mut setup = match boxed_cli_setup_with_retained_skills(
                    args,
                    session.cwd,
                    model,
                    session.provider_factory.metadata.as_ref(),
                    session.mcp_config,
                    Some(RetainedToolSetup {
                        tools: tools.clone(),
                        owner: tool_owner.clone(),
                        reset: false,
                        fresh_edit_session: false,
                        plan_write_selection: None,
                        context_controller: None,
                        projector: Some(sink.projector.clone()),
                        projection_journal: Some(sink.journal.clone()),
                    }),
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
                if let Some(owner) = &tool_owner.native {
                    let preparation = (|| {
                        let options = native_setup_options(&setup, &owner.prepared, &[])?;
                        native_setup::prepare_cli_native(
                            &mut setup,
                            owner.prepared.clone(),
                            Vec::new(),
                            session.cwd,
                            model,
                            options,
                        )
                    })();
                    match preparation {
                        Ok(publication) => setup.native_publication = Some(publication),
                        Err(error) => {
                            eprintln!("ara: could not clear context ({error:#}); session kept");
                            continue;
                        }
                    }
                }
                let factory = session.provider_factory;
                let fresh_session = uuid::Uuid::now_v7().to_string();
                let mut guard = sink.journal.lock().await;
                if let Some(journal) = guard.as_mut()
                    && let Err(error) = journal.append_reset_boundary()
                {
                    eprintln!("ara: could not clear context ({error:#}); session kept");
                    continue;
                }
                if let Some(publication) = &setup.native_publication {
                    if let Err(error) = publication
                        .prepared
                        .publish_catalog_snapshot(publication.pending_snapshot.clone(), None)
                    {
                        sink.persistence_failure(&error);
                        return Err(error.into());
                    }
                }
                let logical_session = guard
                    .as_ref()
                    .map(|journal| journal.session_id().to_owned());
                let fresh_provider = factory.bind_codex_session(fresh_session, logical_session);
                context.clear();
                provider = fresh_provider;
                system_prompt = setup.system_prompt;
                hooks = setup.hooks;
                tools = setup.tools;
                tool_owner = setup.tool_owner;
                task_print::install(args,session.cwd,session.dir.clone(),session.provider_factory,&tool_owner,session.native_jobs.clone(),session.mcp_config,&sink_owner).await?;
                sink.set_edit_mode(tool_owner.edit_policy.tool_mode);
                turn = 0;
                eprintln!("ara: context cleared; original Session history kept");
                continue;
            }
            "/new" => {
                let prepared = async {
                    let mut setup = boxed_cli_setup_with_retained_skills(
                        args,
                        session.cwd,
                        model,
                        session.provider_factory.metadata.as_ref(),
                        session.mcp_config,
                        Some(RetainedToolSetup {
                            tools: tools.clone(),
                            owner: tool_owner.clone(),
                            reset: true,
                            fresh_edit_session: true,
                            plan_write_selection: None,
                            context_controller: None,
                            projector: None,
                            projection_journal: None,
                        }),
                        ToolOverlay::default(),
                        Some(session.skills.to_vec()),
                    )
                    .await?;
                    let mut fresh = match &session.dir {
                        Some(dir) => SessionJournal::create_with_blob_directory(
                            dir,
                            session.cwd,
                            &ara_blobs_directory(),
                        )?,
                        None => SessionJournal::in_memory(ephemeral_header(session.cwd, None))?,
                    };
                    fresh.append_model_change(session.model_ref)?;
                    if let Some(owner) = &setup.tool_owner.read_session {
                        owner
                            .bind_session(fresh.session_id())
                            .map_err(|error| anyhow::Error::msg(error.0))?;
                    }
                    if let Some(owner) = &setup.tool_owner.edit_session {
                        owner.bind(
                            &fresh,
                            ara_cli::session_artifacts::SessionArtifacts::for_journal(&fresh),
                        )?;
                    }
                    let draft = ara_cli::native_factory::PreparedNativeSession::prepare()?;
                    let options = native_setup_options(&setup, &draft, &[])?;
                    let publication = native_setup::prepare_cli_native(
                        &mut setup,
                        draft,
                        Vec::new(),
                        session.cwd,
                        model,
                        options,
                    )?;
                    native_setup::remember_session_jobs(
                        &session.native_jobs,
                        &publication.native_owner,
                    );
                    bind_native_journal(
                        &setup.projector,
                        &mut fresh,
                        publication.prepared.values(),
                    )?;
                    let fresh_router = ara_cli::session_artifacts::ArtifactUriRouter::new(
                        ara_cli::session_artifacts::SessionArtifacts::for_journal(&fresh),
                        prepared_host_uri(
                            setup.tool_owner.vault.as_ref(),
                            setup.tool_owner.uri_base.as_ref(),
                            setup.tool_owner.uri_peer.as_ref(),
                            None,
                        ),
                    );
                    setup.tool_owner.uri_artifacts = Some(fresh_router.clone());
                    if let Some(context) = &setup.tool_owner.context {
                        context.set_uri_port(fresh_router.clone());
                    }
                    *setup.projection_journal.lock().await = Some(fresh);
                    // Preparation failure belongs to the unpublished candidate.
                    // It must not cancel the currently admitted Session.
                    let mut fresh_sink = HostSink::new(
                        args.mode,
                        args.mode == Mode::Text,
                        None,
                        cancel.child_token(),
                        setup.tool_owner.edit_policy.tool_mode,
                        fresh_router.clone(),
                    )
                    .with_terminal_presentation(rpc_host_settings::terminal_presentation_settings(
                        session.cwd,
                        &ara_home().join("agent"),
                    )?)
                    .with_projection_owner(
                        setup.projector.clone(),
                        setup.projection_journal.clone(),
                    )
                    .with_observed_usage(sink.observed_usage.clone())
                    .with_edit_session(setup.tool_owner.edit_session.clone())
                    .with_wait_session(setup.tool_owner.native.as_ref().map(|owner| owner.wait_session.clone()))
                    .with_service_session(setup.tool_owner.native.as_ref().and_then(|owner| owner.service_session.clone()));
                    let mut fresh_thinking = thinking.clone();
                    fresh_thinking.new_session(&fresh_sink).await?;
                    fresh_sink.cancel = cancel.clone();
                    let fresh_sink = Arc::new(fresh_sink);
                    publication
                        .prepared
                        .publish_catalog_snapshot(publication.pending_snapshot.clone(), None)?;
                    Ok::<_, anyhow::Error>((setup, fresh_sink, fresh_thinking, fresh_router))
                }
                .await;
                let (setup, fresh_sink, fresh_thinking, fresh_router) = match prepared {
                    Ok(prepared) => prepared,
                    Err(error) => {
                        eprintln!(
                            "ara: could not start a new session ({error:#}); keeping the current one"
                        );
                        continue;
                    }
                };
                if let Some(host)=sink.task_host(){host.close().await?;}
                if let Some(previous) = &tool_owner.native {
                    // Fixed AgentSession.newSession cancels this Session's jobs.
                    // Join effects while their original sink/journal still exist.
                    native_setup::close_bash_owner(&previous.bash_session).await?;
                }
                let old_path = sink
                    .journal
                    .lock()
                    .await
                    .as_ref()
                    .filter(|j| j.is_persistent())
                    .map(|j| j.path().to_path_buf());
                let guard = fresh_sink.journal.lock().await;
                let journal = guard.as_ref().context("new Session unavailable")?;
                if model.api == "openai-codex-responses" {
                    provider = session
                        .provider_factory
                        .bind_codex_session(journal.session_id().to_owned(), None);
                } else {
                    session.provider_factory.admit_reset_session(
                        model,
                        Some(journal.session_id().to_owned()),
                        None,
                    );
                }
                let header = journal.header().clone();
                drop(guard);
                if args.mode == Mode::Json {
                    fresh_sink.write_line(&header.to_string());
                }
                if let Some(service) = &sink.service_session { service.close(); }
                context.clear();
                system_prompt = setup.system_prompt;
                hooks = setup.hooks;
                tools = setup.tools;
                // OMP b91 sdk.ts session-change callback clears the store the
                // tools actually snapshot into, after successful adoption.
                if let Some(previous) = &tool_owner.context {
                    previous.edit_store.clear();
                }
                if let Some(current) = &setup.tool_owner.context {
                    current.edit_store.clear();
                }
                tool_owner = setup.tool_owner;
                thinking = fresh_thinking;
                sink_owner = fresh_sink;
                task_print::install(args,session.cwd,session.dir.clone(),session.provider_factory,&tool_owner,session.native_jobs.clone(),session.mcp_config,&sink_owner).await?;
                session.artifact_router = fresh_router;
                turn = 0;
                if let Some(path) = old_path {
                    eprintln!(
                        "ara: new session; the previous one stays at {}",
                        path.display()
                    );
                } else {
                    eprintln!("ara: new conversation (not persisted)");
                }
                continue;
            }
            _ => {}
        }
        if input == "/shake" || input.starts_with("/shake ") {
            let mode = input.strip_prefix("/shake").unwrap().trim();
            let mode = if mode.is_empty() { "elide" } else { mode };
            let store = session.artifact_router.current_store();
            let token = cancel.child_token();
            let non_message =
                ara_cli::context_budget::non_message_tokens(model, &system_prompt, &tools);
            let step = run_local_shake(
                mode,
                model,
                session.provider_factory,
                context,
                sink,
                &store,
                non_message,
                &token,
            );
            match interruptible(step, &token, &mut interrupts).await {
                Ok(true) => provider = session.provider_factory.build(),
                Ok(false) => {}
                Err(error) => {
                    eprintln!("ara: shake failed ({error:#}); inspect the Session receipt")
                }
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
                provider: thinking.bind(provider.clone()),
                system_prompt: system_prompt.clone(),
                tools: tools.to_vec(),
                tool_choice: None,
                max_tokens: args.max_tokens,
                temperature: args.temperature,
                deadline: args
                    .max_time
                    .map(|seconds| Instant::now() + Duration::from_secs_f64(seconds.max(0.0))),
                max_model_calls: args.max_model_calls,
                hooks: hooks.clone(),
                interrupt_mode: rpc_host_settings::load_interrupt_mode(&ara_home().join("agent"))?,
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
                    let id = sink
                        .journal
                        .lock()
                        .await
                        .as_ref()
                        .context("Session unavailable")?
                        .session_id()
                        .to_owned();
                    provider = session.provider_factory.bind_codex_session(id, None);
                }
                Ok(false) => {}
                Err(error) => {
                    eprintln!("ara: handoff failed ({error:#}); inspect the Session receipt")
                }
            }
            if sink.persist_failed.load(Ordering::SeqCst) {
                break 1;
            }
            continue;
        }
        turn += 1;
        eprintln!("Working... (turn {turn})");
        let mut config = AgentConfig {
            model: model.clone(),
            provider: thinking.bind(provider.clone()),
            system_prompt: system_prompt.to_vec(),
            tools: tools.to_vec(),
            tool_choice: None,
            max_tokens: args.max_tokens,
            temperature: args.temperature,
            deadline: args
                .max_time
                .map(|s| Instant::now() + Duration::from_secs_f64(s.max(0.0))),
            max_model_calls: args.max_model_calls,
            hooks: hooks.clone(),
            interrupt_mode: rpc_host_settings::load_interrupt_mode(&ara_home().join("agent"))?,
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
                eprintln!(
                    "ara: failed to load skill ({}); session kept",
                    sanitize_text(&error)
                );
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
        let raw_input = match &skill {
            Some(prompt) => sink.projector.new_custom_input(prompt.event_message())?,
            None => sink.projector.new_legacy_message(
                serde_json::to_value(Message::User(UserMessage::text(input)))?,
                None,
            )?,
        };
        let classifier_prompt = skill
            .as_ref()
            .map_or_else(|| draft.to_owned(), |skill| skill.content.plain_text());
        let step = thinking.prepare_turn(
            args,
            session.provider_factory,
            session.cwd,
            classifier_prompt,
            sink,
            &token,
        );
        interruptible(step, &token, &mut interrupts).await?;
        if sink.persist_failed.load(Ordering::SeqCst) {
            break 1;
        }
        if token.is_cancelled() {
            continue;
        }
        config.provider = thinking.bind(provider.clone());
        host_edit_post_apply::install_cli(
            args,
            session.provider_factory,
            session.cwd,
            &tool_owner,
            &config.model,
            &config.hooks,
            sink,
        )
        .await;
        let edit_guard = make_streaming_edit_guard(&tool_owner, Some(&ara_home().join("agent")))?;
        let step = run_host_agent(
            vec![raw_input],
            context,
            &config,
            &token,
            sink.as_ref(),
            tool_owner.native.as_ref(),
            edit_guard,
        );
        let report = interruptible(step, &token, &mut interrupts).await?;
        // Text mode streamed the answer while the turn ran.
        sink.end_stream_line();
        match report.end {
            RunEnd::Completed => {
                if let Some(e) = last_assistant(context, &sink.projector)?
                    .and_then(|a| a.error_message)
                    .as_deref()
                {
                    eprintln!("{}", sanitize_text(e));
                }
            }
            RunEnd::TerminalToolResult => {}
            RunEnd::PreModelStop => {
                eprintln!("ara: turn {turn} stopped before the model call; session kept")
            }
            RunEnd::Aborted => {
                eprintln!("ara: turn {turn} cancelled; session kept, type the next prompt")
            }
            RunEnd::Deadline => eprintln!("ara: turn {turn} hit the deadline; session kept"),
            RunEnd::ModelCallBudget => {
                eprintln!(
                    "ara: model call limit reached ({})",
                    args.max_model_calls.unwrap_or_default()
                )
            }
            RunEnd::Error => {
                // Show this turn's provider cause, as print mode does.
                let cause = last_assistant(
                    context.get(turn_start..).unwrap_or_default(),
                    &sink.projector,
                )?
                .filter(|a| matches!(a.stop_reason, StopReason::Error))
                .and_then(|a| a.error_message)
                .as_deref()
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
            let step = run_compaction(
                args,
                &config,
                session.provider_factory,
                context,
                sink,
                &token,
                false,
            );
            match interruptible(step, &token, &mut interrupts).await {
                Ok(true) => {
                    let id = sink
                        .journal
                        .lock()
                        .await
                        .as_ref()
                        .context("Session unavailable")?
                        .session_id()
                        .to_owned();
                    provider = session.provider_factory.bind_codex_session(id, None);
                }
                Ok(false) => {}
                Err(e) => {
                    eprintln!("ara: auto-compaction failed ({e:#}); inspect the Session receipt")
                }
            }
            if sink.persist_failed.load(Ordering::SeqCst) {
                break 1;
            }
        }
    };
    if let Some(host)=sink_owner.task_host(){host.close().await?;}
    if let Some(lsp) = &tool_owner.lsp_startup {
        if let Err(error) = lsp.shutdown().await {
            eprintln!("ara: LSP shutdown failed: {error}");
        }
    }
    if let Some(path) = sink_owner
        .journal
        .lock()
        .await
        .as_ref()
        .filter(|j| j.is_persistent())
        .map(|j| j.path().to_path_buf())
        && path.exists()
    {
        eprintln!("ara: session {}", path.display());
    }
    Ok(code)
}

fn recover_session(j: &mut SessionJournal) -> Result<()> {
    let display_ids = |ids: &[ara_rpc::WireString]| {
        ara_rpc::WireValue::Array(
            ids.iter()
                .cloned()
                .map(ara_rpc::WireValue::String)
                .collect(),
        )
        .stringify()
    };
    if !j.report.blob_warnings.is_empty() {
        eprintln!(
            "ara: {} missing or malformed image blob reference(s); restore the Session blob store to recover those images",
            j.report.blob_warnings.len()
        );
    }
    if j.report.malformed_records > 0 {
        eprintln!(
            "ara: skipped {} malformed record(s) in {}",
            j.report.malformed_records,
            j.path().display()
        );
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
            display_ids(&recovery.paired)
        );
    }
    if !recovery.unpaired_earlier.is_empty() {
        eprintln!(
            "ara: earlier tool call(s) {} have no recorded result",
            display_ids(&recovery.unpaired_earlier)
        );
    }
    Ok(())
}

struct CliSetup {
    explicit_tools: bool,
    xdev_prompt: Option<String>,
    system_prompt: Vec<String>,
    tools: Vec<Arc<dyn ara_agent::AgentTool>>,
    hooks: Arc<dyn LoopHooks>,
    skills: Vec<ara_discovery::LoadedSkill>,
    tool_owner: CliToolOwner,
    context_controller: ara_cli::append_only_context_mode::SessionContextController,
    projector: Arc<ara_cli::agent_message_projection::AgentMessageProjector>,
    projection_journal: Arc<tokio::sync::Mutex<Option<SessionJournal>>>,
    native_publication: Option<native_setup::NativeSetupPublication>,
    bind_native_tool_context: bool,
}

/// Host-created builtin identity and the immutable modes of this tool snapshot.
/// Cloning retains the Session EditStore/Skill/URI owners; modes are plain fields.
#[derive(Clone)]
struct CliToolOwner {
    read_video: Option<Arc<ara_cli::host_read_video::HostReadVideo>>,
    read_video_policy: Option<Arc<ara_cli::host_read_video::HostReadVideoPolicy>>,
    read_host: Option<Arc<ara_cli::host_browser::HostBrowser>>,
    read_session: Option<Arc<ara_cli::host_browser::HostReadSession>>,
    lsp_startup: Option<Arc<ara_cli::host_lsp::HostLspStartup>>,
    vault: Option<Arc<ara_cli::host_vault::HostVault>>,
    /// Explicit Host/plan routing skeleton; it never contains a vault fallback.
    uri_base: Option<Arc<dyn ara_tools::ContentUriPort>>,
    uri_peer:Option<Arc<dyn ara_tools::ContentUriPort>>,
    uri_artifacts: Option<Arc<ara_cli::session_artifacts::ArtifactUriRouter>>,
    edit_session: Option<Arc<ara_cli::host_edit_session::HostEditSession>>,
    context: Option<ToolContext>,
    owned_builtins: Vec<Arc<dyn ara_agent::AgentTool>>,
    /// Original configured transport owners; Native preparation reuses these exact Arcs.
    owned_mcp: Vec<Arc<ara_mcp::McpTool>>,
    native: Option<Arc<native_setup::NativeCliOwner>>,
    edit_policy: edit_mode_policy::ResolvedEditMode,
}

impl Default for CliToolOwner {
    fn default() -> Self {
        Self {
            read_host: None,
            read_video: None,
            read_video_policy: None,
            read_session: None,
            lsp_startup: None,
            vault: None,
            uri_base: None,
            uri_peer:None,
            uri_artifacts: None,
            edit_session: None,
            context: None,
            owned_builtins: Vec::new(),
            owned_mcp: Vec::new(),
            native: None,
            edit_policy: edit_mode_policy::ResolvedEditMode {
                tool_mode: ara_edit::EditMode::Hashline,
                display_mode: ara_edit::EditMode::Hashline,
            },
        }
    }
}

struct RetainedToolSetup {
    tools: Vec<Arc<dyn ara_agent::AgentTool>>,
    owner: CliToolOwner,
    reset: bool,
    fresh_edit_session: bool,
    /// ACP temporarily exposes only its own missing WRITE transport. None
    /// preserves all ordinary selected/pinned tools and other Host modes.
    plan_write_selection: Option<bool>,
    context_controller: Option<ara_cli::append_only_context_mode::SessionContextController>,
    projector: Option<Arc<ara_cli::agent_message_projection::AgentMessageProjector>>,
    projection_journal: Option<Arc<tokio::sync::Mutex<Option<SessionJournal>>>>,
}

// A selected-model owner is captured per admitted tool catalog. The current
// setting/environment is read by the same tool instance before each call.
struct LiveEditPolicyReader {
    read: Box<
        dyn Fn()
                -> Result<ara_tools::edit_runtime_policy::EditRuntimeSnapshot, ara_agent::ToolError>
            + Send
            + Sync,
    >,
    identity: String,
}
impl ara_tools::edit_runtime_policy::EditRuntimePolicy for LiveEditPolicyReader {
    fn snapshot(
        &self,
    ) -> Result<ara_tools::edit_runtime_policy::EditRuntimeSnapshot, ara_agent::ToolError> {
        (self.read)()
    }
    fn reader_identity(&self) -> Option<String> {
        Some(self.identity.clone())
    }
}
fn live_edit_policy(
    cli_mode: Option<ara_edit::EditMode>,
    metadata: serde_json::Value,
    edit_enabled: bool,
) -> Arc<dyn ara_tools::edit_runtime_policy::EditRuntimePolicy> {
    let agent_dir = ara_home().join("agent");
    let identity = serde_json::to_string(&serde_json::json!({"model":&metadata,
        "cli":format!("{cli_mode:?}"),"editEnabled":edit_enabled,"agentDir":&agent_dir}))
    .expect("Host EDIT reader identity serializes");
    Arc::new(LiveEditPolicyReader {
        identity,
        read: Box::new(move || {
            let read = || -> anyhow::Result<ara_tools::edit_runtime_policy::EditRuntimeSnapshot> {
                let variant = std::env::var("ARA_EDIT_VARIANT").ok();
                let strict = std::env::var("ARA_STRICT_EDIT_MODE").ok();
                let fuzzy = std::env::var("ARA_EDIT_FUZZY").ok();
                let threshold = std::env::var("ARA_EDIT_FUZZY_THRESHOLD").ok();
                let resolved = edit_mode_policy::resolve_edit_mode(
                    cli_mode,
                    &rpc_host_settings::edit_mode_settings(&agent_dir)?,
                    edit_mode_policy::EditModeEnvironment {
                        variant: variant.as_deref(),
                        strict: strict.as_deref(),
                    },
                    Some(&metadata),
                    edit_enabled,
                )?;
                let settings = if edit_enabled {
                    edit_mode_policy::resolve_edit_settings(
                        &rpc_host_settings::edit_tool_settings(&agent_dir)?,
                        edit_mode_policy::EditSettingsEnvironment {
                            fuzzy: fuzzy.as_deref(),
                            fuzzy_threshold: threshold.as_deref(),
                        },
                    )?
                } else {
                    Default::default()
                };
                Ok(ara_tools::edit_runtime_policy::EditRuntimeSnapshot {
                    mode: resolved.tool_mode,
                    display_mode: resolved.display_mode,
                    settings,
                    compact_prompt: metadata
                        .get("editPromptVariant")
                        .and_then(serde_json::Value::as_str)
                        == Some("compact"),
                })
            };
            read().map_err(|error| ara_agent::ToolError(error.to_string()))
        }),
    })
}

fn new_tool_context(
    cwd: &Path,
    line_numbers: bool,
    read_settings: rpc_host_settings::ReadSettings,
    skills: &[ara_discovery::LoadedSkill],
    policy: edit_mode_policy::ResolvedEditMode,
    edit_settings: ara_tools::edit::EditSettings,
    runtime_policy: Arc<dyn ara_tools::edit_runtime_policy::EditRuntimePolicy>,
    edit_enabled: bool,
    edit_session: &Arc<ara_cli::host_edit_session::HostEditSession>,
    read_session: Option<&Arc<ara_cli::host_browser::HostReadSession>>,
    read_video: Option<&Arc<ara_cli::host_read_video::HostReadVideo>>,
    read_video_policy: Option<&Arc<ara_cli::host_read_video::HostReadVideoPolicy>>,
) -> ToolContext {
    let mut context = ara_cli::archive_date::bind(ToolContext::new(cwd.to_path_buf()))
        .with_read_text_metrics(Arc::new(
            ara_cli::terminal::render_utils::TerminalReadTextMetrics,
        ))
        .with_read_remote_mount_root(ara_home().join("remote"))
        .with_document_cache(Arc::new(
            ara_tools::read_document_cache::DocumentCache::new(
                ara_home().join("cache").join("markit"),
                "18.1.8".into(),
            ),
        ))
        .with_read_output_max_columns(read_settings.output_max_columns)
        .with_read_default_limit(read_settings.default_limit)
        .with_read_render_markdown(read_settings.render_markdown)
        .with_read_auto_resize_images(read_settings.auto_resize_images)
        .with_read_summary_policy(read_settings.summary)
        .with_edit_modes(policy.tool_mode, policy.display_mode, edit_enabled)
        .with_edit_settings(edit_settings)
        .with_edit_runtime_policy(runtime_policy)
        .with_edit_host(edit_session.clone())
        .with_read_plan_reference_port(edit_session.clone())
        .with_skills(
            skills
                .iter()
                .map(|skill| ara_tools::internal_urls::SkillRef {
                    name: skill.name.clone(),
                    file_path: skill.file_path.clone(),
                    base_dir: skill.base_dir.clone(),
                })
                .collect(),
        );
    if let Some(owner) = read_session {
        context = owner.bind_ports(context);
    }
    if let Some(owner) = read_video {
        context = owner.bind_ports(
            context,
            read_session
                .is_none()
                .then(|| read_video_policy.cloned())
                .flatten(),
        );
    }
    context.line_numbers = line_numbers;
    ara_cli::host_grammars::HostGrammars::install(
        &context,
        ara_home().join("cache").join("grammars"),
    );
    context
}

fn prepared_host_uri(
    vault: Option<&Arc<ara_cli::host_vault::HostVault>>,
    base: Option<&Arc<dyn ara_tools::ContentUriPort>>,
    peer:Option<&Arc<dyn ara_tools::ContentUriPort>>,
    artifacts: Option<&Arc<ara_cli::session_artifacts::ArtifactUriRouter>>,
) -> Option<Arc<dyn ara_tools::ContentUriPort>> {
    let vault = vault.map(|vault| vault.clone() as Arc<dyn ara_tools::ContentUriPort>);
    // Explicit Registered/Removed routes always win; current peer fallback is
    // separate from the stable base and never nests an older peer namespace.
    let native=peer.map(|peer|ara_cli::host_vault::compose_ports(peer.clone(),vault.clone())).or(vault);
    let fallback = match base {
        Some(base) => Some(ara_cli::host_vault::compose_ports(base.clone(), native)),
        None => native,
    };
    match artifacts {
        Some(artifacts) => Some(artifacts.fork_with_fallback(fallback)),
        None => fallback,
    }
}

fn owned_builtin_tools(
    context: &ToolContext,
    enabled: &[&str],
) -> Vec<Arc<dyn ara_agent::AgentTool>> {
    let mut tools: Vec<_> = builtin_tools(context.clone())
        .into_iter()
        .filter(|tool| enabled.contains(&tool.definition().name.as_str()))
        .collect();
    if enabled.contains(&"ast_edit") {
        tools.push(Arc::new(ara_tools::ast_edit::AstEditTool {
            ctx: context.clone(),
        }));
    }
    if enabled.contains(&"ast_grep") {
        tools.push(Arc::new(ara_tools::ast_grep::AstGrepTool {
            ctx: context.clone(),
        }));
    }
    if enabled.contains(&"debug") {
        tools.push(Arc::new(ara_tools::default_declarations::DebugTool));
    }
    if enabled.contains(&"wait"){tools.push(Arc::new(ara_cli::actual_tools::wait_native::WaitTool));}
    if enabled.contains(&"task"){tools.push(Arc::new(ara_cli::actual_tools::task_native::TaskTool));}
    if enabled.contains(&"web_search") {
        tools.push(Arc::new(ara_tools::default_declarations::WebSearchTool));
    }
    tools
}

/// Prepare another logical ACP Session through the same production route,
/// tool, Native realm and journal owners used by CLI/RPC startup.
async fn prepare_acp_seed(
    template: &rpc_host::SessionFactory,
    cwd: &Path,
    stored: Option<SessionJournal>,
    connection: &CancellationToken,
) -> Result<acp_host::NativeSeed> {
    let mut args = template.args.clone();
    args.cwd = Some(cwd.to_owned());
    args.mode = Mode::Acp;
    args.resume = None;
    args.continue_session = false;
    let mut shared_account = template.provider.shared_account.clone();
    let mut route = resolve_startup_route(
        &mut args,
        cwd,
        &mut shared_account,
        connection,
        stored.as_ref(),
    )
    .await?;
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
    let session_dir = args
        .session_dir
        .clone()
        .unwrap_or_else(|| default_session_dir(cwd));
    let mut journal = match stored {
        Some(mut journal) => {
            journal.bind_blob_directory(&ara_blobs_directory())?;
            recover_session(&mut journal)?;
            journal
        }
        None => {
            SessionJournal::create_with_blob_directory(&session_dir, cwd, &ara_blobs_directory())?
        }
    };
    if journal.current_model()?.is_none() {
        journal.append_model_change(&format!("{}/{}", route.model.provider, route.model.id))?;
    }
    journal.materialize()?;
    let header = journal.header().clone();
    let mut setup = Box::pin(prepare_cli_setup(
        &args,
        cwd,
        &route.model,
        route.daily.as_ref().map(|selection| &selection.metadata),
        &template.mcp_config,
        None,
        ToolOverlay {
            peer_task_depth: rpc_host_settings::peer_task_depth(Some(&journal))?,
            peer_enable_irc: Some(template.peer_messaging_enabled),
            read_host: template.tool_owner.read_host.clone(),
            read_video: template.tool_owner.read_video.clone(),
            ..ToolOverlay::default()
        },
    ))
    .await?;
    if let Some(owner) = &setup.tool_owner.read_session {
        owner
            .bind_session(journal.session_id())
            .map_err(|error| anyhow::Error::msg(error.0))?;
    }
    if let Some(owner) = &setup.tool_owner.edit_session {
        owner.bind(
            &journal,
            ara_cli::session_artifacts::SessionArtifacts::for_journal(&journal),
        )?;
    }
    let draft = ara_cli::native_factory::PreparedNativeSession::prepare()?;
    let options = native_setup_options(&setup, &draft, &[])?;
    let publication = native_setup::prepare_cli_native(
        &mut setup,
        draft,
        Vec::new(),
        cwd,
        &route.model,
        options,
    )?;
    native_setup::remember_session_jobs(&template.native_jobs, &publication.native_owner);
    let native_session = publication.prepared.clone();
    native_session.publish_catalog_snapshot(publication.pending_snapshot.clone(), None)?;
    bind_native_journal(&setup.projector, &mut journal, native_session.values())?;
    let provider = if let Some(selection) = route.daily.take() {
        ProviderFactory::daily(
            template.provider.client.clone(),
            selection,
            header["id"].as_str().map(str::to_owned),
            cwd,
            connection,
            shared_account,
            route.registry.clone(),
        )
        .await?
    } else {
        ProviderFactory::startup(
            template.provider.client.clone(),
            route.model.clone(),
            selected_api,
            route.stream_options,
            args.anthropic_strict_tools,
            args.responses_stateful,
        )?
    };
    let messages = provider.context_for(&journal, &setup.projector)?;
    let config = AgentConfig {
        model: route.model,
        provider: provider.build(),
        system_prompt: setup.system_prompt,
        tools: setup.tools,
        tool_choice: None,
        max_tokens: args.max_tokens,
        temperature: args.temperature,
        deadline: None,
        max_model_calls: args.max_model_calls,
        hooks: setup.hooks,
        interrupt_mode: rpc_host_settings::load_interrupt_mode(&ara_home().join("agent"))?,
    };
    let max_time = args.max_time;
    let sessions = rpc_host::SessionFactory {
        peer_messaging_enabled: template.peer_messaging_enabled,
            agent_context: None,
        dir: Some(session_dir),
        cwd: cwd.to_owned(),
        provider,
        args,
        explicit_generation: template.explicit_generation.clone(),
        mcp_config: template.mcp_config.clone(),
        tool_owner: setup.tool_owner,
        uri_port: None,
        startup: route.startup,
        context_controller: Some(setup.context_controller),
        actual_values_factory: Some(Arc::new(|| {
            Ok(rpc_host::PreparedActualSession::Native(
                ara_cli::native_factory::PreparedNativeSession::prepare()?,
            ))
        })),
        initial_native_session: Some(native_session),
        native_jobs: template.native_jobs.clone(),
        peers: template.peers.clone(),
        peer_root: template.peer_root.clone(),
        peer_attachment:None,
        acp_plan: None,
    };
    Ok(acp_host::NativeSeed {
        config,
        skills: setup.skills,
        messages,
        journal: Some(journal),
        header,
        max_time,
        sessions,
    })
}

#[allow(
    clippy::too_many_arguments,
    reason = "Retained owners and mode/settings snapshots remain distinct Host inputs."
)]
fn rebind_owned_tools(
    retained: RetainedToolSetup,
    cwd: &Path,
    line_numbers: bool,
    read_settings: rpc_host_settings::ReadSettings,
    skills: &[ara_discovery::LoadedSkill],
    policy: edit_mode_policy::ResolvedEditMode,
    edit_settings: ara_tools::edit::EditSettings,
    runtime_policy: Arc<dyn ara_tools::edit_runtime_policy::EditRuntimePolicy>,
    enabled: &[&str],
    refresh_skills: bool,
    lsp_startup: Arc<ara_cli::host_lsp::HostLspStartup>,
    vault: Arc<ara_cli::host_vault::HostVault>,
    read_host: Option<Arc<ara_cli::host_browser::HostBrowser>>,
    read_session: Option<Arc<ara_cli::host_browser::HostReadSession>>,
    read_video: Option<Arc<ara_cli::host_read_video::HostReadVideo>>,
    read_video_policy: Option<Arc<ara_cli::host_read_video::HostReadVideoPolicy>>,
) -> (Vec<Arc<dyn ara_agent::AgentTool>>, CliToolOwner) {
    let settings_unchanged = retained.owner.context.as_ref().is_none_or(|context| {
        context.edit_settings() == edit_settings && read_settings.matches(context)
    });
    let runtime_unchanged = retained.owner.context.as_ref().is_none_or(|context| {
        let identity = runtime_policy.reader_identity();
        identity.is_some()
            && context.edit_runtime_reader_identity() == identity
            && matches!((context.try_edit_runtime_policy(), runtime_policy.snapshot()),
                (Ok(previous), Ok(current)) if previous == current)
    });
    let lsp_unchanged = retained
        .owner
        .lsp_startup
        .as_ref()
        .is_some_and(|owner| owner.snapshot == lsp_startup.snapshot);
    let vault_unchanged = retained
        .owner
        .vault
        .as_ref()
        .is_some_and(|owner| owner.enabled() == vault.enabled());
    let write_unchanged = retained.plan_write_selection.is_none_or(|selected| {
        selected
            == retained
                .tools
                .iter()
                .any(|tool| tool.definition().name == "write")
    });
    let media_unchanged = match (&retained.owner.read_video, &read_video) {
        (None, None) => true,
        (Some(previous), Some(current)) => Arc::ptr_eq(previous, current),
        _ => false,
    } && match (&retained.owner.read_video_policy, &read_video_policy) {
        (None, None) => true,
        (Some(previous), Some(current)) => previous.same_snapshot(current),
        _ => false,
    };
    if read_session.is_none()
        && media_unchanged
        && !retained.reset
        && lsp_unchanged
        && vault_unchanged
        && !retained.fresh_edit_session
        && !refresh_skills
        && retained.owner.edit_policy == policy
        && settings_unchanged
        && runtime_unchanged
        && write_unchanged
    {
        return (retained.tools, retained.owner);
    }
    let mut supplied = lsp_startup.supplied();
    supplied.vault = Some(vault.clone());
    let edit_session = Some(match &retained.owner.edit_session {
        Some(owner) if retained.reset || retained.fresh_edit_session => {
            owner.fresh_with_supplied(supplied)
        }
        Some(owner) if lsp_unchanged && vault_unchanged => owner.clone(),
        Some(owner) => owner.with_supplied(supplied),
        None => ara_cli::host_edit_session::HostEditSession::new(user_home(), supplied),
    });
    let context = if retained.reset {
        let mut context = new_tool_context(
            cwd,
            line_numbers,
            read_settings,
            skills,
            policy,
            edit_settings,
            runtime_policy.clone(),
            enabled.contains(&"edit"),
            edit_session.as_ref().expect("fresh EDIT Session owner"),
            read_session.as_ref(),
            read_video.as_ref(),
            read_video_policy.as_ref(),
        );
        // Preparation preserves the existing EditStore until Session adoption
        // commits. A failed new/switch must leave its issued tags intact.
        if let Some(previous) = &retained.owner.context {
            context.edit_store = previous.edit_store.clone();
        }
        Some(context)
    } else {
        retained.owner.context.clone().map(|context| {
            let context = if retained.fresh_edit_session || !lsp_unchanged || !vault_unchanged {
                context
                    .with_edit_host(
                        edit_session
                            .as_ref()
                            .expect("fresh EDIT Session owner")
                            .clone(),
                    )
                    .with_read_plan_reference_port(
                        edit_session
                            .as_ref()
                            .expect("fresh EDIT Session owner")
                            .clone(),
                    )
            } else {
                context
            };
            let context = context
                .with_edit_modes(
                    policy.tool_mode,
                    policy.display_mode,
                    enabled.contains(&"edit"),
                )
                .with_edit_settings(edit_settings)
                .with_edit_runtime_policy(runtime_policy.clone())
                .with_read_output_max_columns(read_settings.output_max_columns)
                .with_read_default_limit(read_settings.default_limit)
                .with_read_render_markdown(read_settings.render_markdown)
                .with_read_auto_resize_images(read_settings.auto_resize_images)
                .with_read_summary_policy(read_settings.summary);
            if refresh_skills {
                // Replace the candidate's Skill Arc, not its shared contents.
                // Admitted tool snapshots retain their original Skill owner.
                context.with_skills(
                    skills
                        .iter()
                        .map(|skill| ara_tools::internal_urls::SkillRef {
                            name: skill.name.clone(),
                            file_path: skill.file_path.clone(),
                            base_dir: skill.base_dir.clone(),
                        })
                        .collect(),
                )
            } else {
                context
            }
        })
    };
    let context = context.map(|context| match &read_session {
        Some(owner) => owner.bind_ports(context),
        None => context,
    });
    let context = context.map(|context| match &read_video {
        Some(owner) => owner.bind_ports(
            context,
            read_session
                .is_none()
                .then(|| read_video_policy.clone())
                .flatten(),
        ),
        None => context,
    });
    // Fork the slot before constructing adapters. Failed Native preparation
    // cannot mutate the live slot; disabled snapshots use a new vault gate.
    // Rebuild from the stable explicit/artifact skeleton, never an old vault.
    let context = context.map(|context| {
        context.with_isolated_uri_port(prepared_host_uri(
            Some(&vault),
            retained.owner.uri_base.as_ref(),
            retained.owner.uri_peer.as_ref(),
            retained.owner.uri_artifacts.as_ref(),
        ))
    });
    let replacements = context
        .as_ref()
        .map(|context| owned_builtin_tools(context, enabled))
        .unwrap_or_default();
    let replace_slot = |original: &Arc<dyn ara_agent::AgentTool>| {
        let owned = retained
            .owner
            .owned_builtins
            .iter()
            .any(|owned| Arc::ptr_eq(owned, original));
        let mode_dependent = ["read", "write", "edit", "grep", "ast_grep", "ast_edit"]
            .contains(&original.definition().name.as_str());
        if owned
            && (retained.reset || retained.fresh_edit_session || refresh_skills || mode_dependent)
        {
            replacements
                .iter()
                .find(|replacement| replacement.definition().name == original.definition().name)
                .cloned()
                .unwrap_or_else(|| original.clone())
        } else {
            original.clone()
        }
    };
    // Publish the new provenance with the new tools. Subsequent transitions must
    // recognize these new Arcs while old admitted snapshots keep their originals.
    let retain_write = |tool: &&Arc<dyn ara_agent::AgentTool>| {
        !(retained.plan_write_selection == Some(false)
            && tool.definition().name == "write"
            && retained
                .owner
                .owned_builtins
                .iter()
                .any(|owned| Arc::ptr_eq(owned, tool)))
    };
    let mut tools = retained
        .tools
        .iter()
        .filter(retain_write)
        .map(&replace_slot)
        .collect::<Vec<_>>();
    let mut owned_builtins = retained
        .owner
        .owned_builtins
        .iter()
        .filter(retain_write)
        .map(replace_slot)
        .collect::<Vec<_>>();
    if retained.plan_write_selection == Some(true)
        && !tools.iter().any(|tool| tool.definition().name == "write")
    {
        if let Some(write) = replacements
            .iter()
            .find(|tool| tool.definition().name == "write")
        {
            tools.push(write.clone());
            owned_builtins.push(write.clone());
        }
    }
    (
        tools,
        CliToolOwner {
            read_host,
            read_session,
            read_video,
            read_video_policy,
            lsp_startup: Some(lsp_startup),
            vault: Some(vault),
            uri_base: retained.owner.uri_base,
            uri_peer:retained.owner.uri_peer,
            uri_artifacts: retained.owner.uri_artifacts,
            edit_session,
            context,
            owned_builtins,
            owned_mcp: retained.owner.owned_mcp,
            native: retained.owner.native,
            edit_policy: policy,
        },
    )
}

#[derive(Default)]
struct ToolOverlay {
    peer_coordination: bool,
    peer_task_depth: usize,
    peer_enable_irc: Option<bool>,
    child_read_summarize:Option<bool>,
    child_restrict_lsp:bool,
    read_video: Option<Arc<ara_cli::host_read_video::HostReadVideo>>,
    read_host: Option<Arc<ara_cli::host_browser::HostBrowser>>,
    tools: Vec<Arc<dyn ara_agent::AgentTool>>,
    native_owners: Vec<(
        Arc<dyn ara_agent::AgentTool>,
        Arc<dyn ara_cli::native_rpc_host::NativeRpcHostOwner>,
    )>,
    labels: std::collections::HashMap<String, String>,
}

async fn prepare_cli_setup(
    args: &Args,
    cwd: &Path,
    model: &Model,
    model_metadata: Option<&serde_json::Value>,
    mcp_config: &Option<McpServerConfig>,
    retained_tools: Option<RetainedToolSetup>,
    overlay: ToolOverlay,
) -> Result<CliSetup> {
    prepare_cli_setup_with_retained_skills(
        args,
        cwd,
        model,
        model_metadata,
        mcp_config,
        retained_tools,
        overlay,
        None,
    )
    .await
}

fn native_owned_env(value: Option<&str>) -> Option<ara_ai::dialect::Dialect> {
    if matches!(value, Some("1" | "true")) {
        return Some(ara_ai::dialect::Dialect::Glm);
    }
    value.and_then(ara_ai::actual_provider::graph_history_demotion_dialect)
}

fn native_setup_options(
    setup: &CliSetup,
    draft: &Arc<ara_cli::native_factory::PreparedNativeSession>,
    captured_rpc: &[(
        Arc<dyn ara_agent::AgentTool>,
        Arc<dyn ara_cli::native_rpc_host::NativeRpcHostOwner>,
    )],
) -> Result<native_setup::NativeSetupHostOptions> {
    use ara_cli::actual_provider::{ActualNativeProviderPort, ActualOwnedDialectSelector};
    let binding = setup.context_controller.binding();
    let port = if binding.owned_port.is_none() {
        let format = setup.context_controller.tool_format();
        let selector = ara_ai::actual_provider::graph_history_demotion_dialect(&format)
            .map(ActualOwnedDialectSelector::Explicit)
            .unwrap_or(ActualOwnedDialectSelector::ModelPreferred);
        Some(Arc::new(
            ActualNativeProviderPort::with_owned_projection(
                selector,
                draft.realm().globals.json.clone(),
                draft.realm().globals.date.clone(),
            )
            .with_owned_stream_globals(draft.realm().globals.environment.clone()),
        ) as Arc<dyn ara_agent::OwnedProviderContextPort>)
    } else {
        None
    };
    let rpc_grants = captured_rpc
        .iter()
        .map(|(_, owner)| {
            // This is the current Host's registered RPC definition, not an inferred
            // source identity. Preserve a revoked grant when that connection rebuilds
            // the same logical registration; unfamiliar registrations use the Host's
            // explicit enabled registry grant below.
            if let Some(prior) = &setup.tool_owner.native {
                if let Some(grant) = prior.rpc_grant(owner)? {
                    return Ok(grant);
                }
            }
            Ok(true)
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(native_setup::NativeSetupHostOptions {
        logger: draft.catalog().is_none().then(|| {
            ara_cli::native_logger::logger_host::NativeLoggerHostOptions {
                default_logs_dir: ara_home().join("agent").join("logs"),
                filename_prefix: "ara".into(),
                date: Arc::new(ara_cli::native_realm::date::SystemDateHost),
            }
        }),
        local_routes: setup
            .tool_owner
            .edit_session
            .as_ref()
            .map(|owner| owner.local_routes(draft))
            .transpose()?
            .unwrap_or_default(),
        bash_settings: ara_cli::actual_tools::bash_native::BashSessionSettings::default(),
        grants: native_setup::NativeSetupGrants {
            builtin: vec![true; setup.tool_owner.owned_builtins.len()],
            mcp: vec![true; setup.tool_owner.owned_mcp.len()],
            rpc: rpc_grants,
        },
        provider_port: port,
        proposal_host: None,
    })
}

// Explicit reference-Host process selection, shared by all Session snapshots.
// No consent handler is installed in print mode; unset means no collection.
static CLI_AUTO_QA: std::sync::OnceLock<Arc<ara_cli::report_tool_issue::AutoQaProcessOwner>> =
    std::sync::OnceLock::new();
fn cli_report_issue_host(
    cwd: &Path,
    model: &Model,
) -> Result<ara_cli::report_tool_issue::ReportIssueHost> {
    use ara_cli::report_tool_issue::*;
    let root = ara_home();
    let settings = rpc_host_settings::AutoQaPolicy::load(cwd, &root.join("agent"))?;
    let owner = CLI_AUTO_QA
        .get_or_init(|| {
            let identity_host =
                ara_cli::auth_broker_discover::BrokerDiscoveryHost::for_process(root.clone(), None);
            AutoQaProcessOwner::new(AutoQaHostPorts {
                database_path: Some(root.join("autoqa.db")),
                version: env!("CARGO_PKG_VERSION").into(),
                platform: match std::env::consts::OS {
                    "windows" => "win32",
                    "macos" => "darwin",
                    other => other,
                }
                .into(),
                arch: match std::env::consts::ARCH {
                    "x86_64" => "x64",
                    "aarch64" => "arm64",
                    other => other,
                }
                .into(),
                install_id: Arc::new(move || {
                    ara_cli::auth_broker_discover::client_identity(&identity_host).install_id
                }),
                clock: Arc::new(SystemAutoQaClock),
                http: Arc::new(ReqwestAutoQaHttp),
            })
        })
        .clone();
    Ok(ReportIssueHost {
        owner,
        settings: Some(settings),
        environment: rpc_host_settings::autoqa_environment(),
        model: format!("{}/{}", model.provider, model.id),
    })
}

fn bind_native_journal(
    projector: &ara_cli::agent_message_projection::AgentMessageProjector,
    journal: &mut SessionJournal,
    values: &Arc<ara_agent::SessionValues>,
) -> Result<()> {
    let encoder = Arc::new(ara_cli::actual_session::GraphJournalEncoder::new(
        values.clone(),
        None,
    )?);
    projector.bind_native_session(journal, encoder)?;
    Ok(())
}

fn make_streaming_edit_guard(
    owner: &CliToolOwner,
    settings_dir: Option<&Path>,
) -> Result<Option<Arc<ara_cli::streaming_edit_guard::StreamingEditGuard>>> {
    // Inspect the admitted builtin owner, not advertisement aliases or a
    // current provider's JSON projection. No Edit means no Edit config I/O.
    if !owner
        .owned_builtins
        .iter()
        .any(|tool| tool.definition().name == "edit")
    {
        return Ok(None);
    }
    let context = owner
        .context
        .as_ref()
        .context("Edit builtin has no Host ToolContext")?;
    let policy = ara_tools::edit::EditTool::new(context.clone()).streaming_policy();
    let streaming_abort = settings_dir
        .map(rpc_host_settings::edit_streaming_abort)
        .transpose()?
        .unwrap_or(false);
    Ok(Some(Arc::new(
        ara_cli::streaming_edit_guard::StreamingEditGuard::new(policy, streaming_abort),
    )))
}

// Allocate the complete Agent/Native driver at an ordinary function boundary,
// keeping its construction temporary out of the REPL's interruptible poll frame.
fn run_host_agent<'a>(
    input: Vec<AgentMessage>,
    context: &'a mut Vec<AgentMessage>,
    config: &'a AgentConfig,
    cancel: &'a CancellationToken,
    sink: &'a HostSink,
    native: Option<&'a Arc<native_setup::NativeCliOwner>>,
    edit_guard: Option<Arc<ara_cli::streaming_edit_guard::StreamingEditGuard>>,
) -> Pin<Box<impl Future<Output = Result<ara_agent::RunReport>> + 'a>> {
    Box::pin(run_host_agent_owned(
        input, context, config, cancel, sink, native, edit_guard,
    ))
}

async fn run_host_agent_owned(
    input: Vec<AgentMessage>,
    context: &mut Vec<AgentMessage>,
    config: &AgentConfig,
    cancel: &CancellationToken,
    sink: &HostSink,
    native: Option<&Arc<native_setup::NativeCliOwner>>,
    edit_guard: Option<Arc<ara_cli::streaming_edit_guard::StreamingEditGuard>>,
) -> Result<ara_agent::RunReport> {
    sink.bind_service_session().await?;
    let _service_claims = ara_cli::host_services::ServiceRunClaims(sink.service_session.clone());
    // Stream validation owns this Run's child token. A generated-file abort
    // must not cancel the reusable REPL/print Host token.
    let run_cancel = cancel.child_token();
    if let Err(error) = sink
        .refresh_terminal_history(native.map(|owner| owner.prepared.clone()))
        .await
    {
        if let Some(terminal) = &sink.terminal {
            terminal.diagnostic(&format!("ara: Session display refresh failed ({error})"));
        }
    }
    // The driver owns terminal restoration even when native admission fails,
    // the Run future errors or unwinds. The sink remains persistence-first.
    let terminal_run = if let Some(terminal) = &sink.terminal {
        let mode = *sink
            .edit_mode
            .read()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(display) = &sink.display {
            let mut display = display.lock().await;
            display.attach_native(native.map(|owner| owner.prepared.clone()))?;
            display.begin_run(mode);
        }
        let generation = terminal
            .begin_run(mode, run_cancel.clone())
            .context("starting terminal Run")?;
        sink.terminal_generation
            .store(generation, Ordering::Release);
        Some(HostTerminalRun {
            sink,
            terminal: terminal.clone(),
            generation,
        })
    } else {
        None
    };
    let _terminal_run = terminal_run;
    let guarded_sink = ara_cli::streaming_edit_sink::StreamingEditSink::new(
        sink,
        edit_guard,
        native.map(|owner| owner.prepared.values().clone()),
        run_cancel.clone(),
    );
    let console_run=match sink.task_host(){Some(host)=>Some(host.begin_run(context,&run_cancel).await?),None=>None};
    let future = agent_loop_raw(input, context, config, &run_cancel, &guarded_sink);
    let result=match native {
        Some(owner) => Ok(ara_cli::native_run::NativeRun::begin(
            owner.prepared.clone(),
            run_cancel.clone(),
        )?
        .drive_with_display(future, sink.display.clone())
        .await?),
        None => {
            tokio::pin!(future);
            loop {
                let more_display = if let Some(display) = &sink.display {
                    display.lock().await.pump_quantum()
                } else {
                    false
                };
                let demand = async {
                    if let Some(terminal) = &sink.terminal {
                        terminal.display_port().ready().await
                    } else {
                        std::future::pending::<()>().await
                    }
                };
                tokio::select! {result=&mut future=>{
                    if let Some(display)=&sink.display{let mut display=display.lock().await;let cutoff=display.port.cutoff();display.finish_through(cutoff);}
                    break Ok(result);
                },_=demand=>{},_=tokio::task::yield_now(),if more_display=>{}}
            }
        }
    };
    if let Some(console_run)=console_run{console_run.finish(&result).await?;}
    result
}

// Keep the complete setup future out of RPC recovery/settlement layouts.
// Construct and allocate at an ordinary function boundary: an async wrapper
// would still retain the large construction temporary in its caller's state.
#[allow(
    clippy::too_many_arguments,
    reason = "Keep the boxed setup allocation boundary and distinct retained host snapshots."
)]
fn boxed_cli_setup_with_retained_skills<'a>(
    args: &'a Args,
    cwd: &'a Path,
    model: &'a Model,
    model_metadata: Option<&'a serde_json::Value>,
    mcp_config: &'a Option<McpServerConfig>,
    retained_tools: Option<RetainedToolSetup>,
    overlay: ToolOverlay,
    retained_skills: Option<Vec<ara_discovery::LoadedSkill>>,
) -> std::pin::Pin<Box<impl std::future::Future<Output = Result<CliSetup>> + 'a>> {
    Box::pin(prepare_cli_setup_with_retained_skills(
        args,
        cwd,
        model,
        model_metadata,
        mcp_config,
        retained_tools,
        overlay,
        retained_skills,
    ))
}

// Retained tools, skills, and the overlay have separate Host snapshot lifetimes.
#[allow(
    clippy::too_many_arguments,
    reason = "Preserve distinct route, tool, Skill, and context inputs to Host setup."
)]
async fn prepare_cli_setup_with_retained_skills(
    args: &Args,
    cwd: &Path,
    model: &Model,
    model_metadata: Option<&serde_json::Value>,
    mcp_config: &Option<McpServerConfig>,
    retained_tools: Option<RetainedToolSetup>,
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
            .map(|s| {
                s.split(',')
                    .map(|p| p.trim().to_string())
                    .filter(|p| !p.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        ..SkillsSettings::default()
    };
    // Context reset refreshes rules and base prompt files, while slash command
    // dispatch, advertised skills and retained tools keep one Skill snapshot.
    let refresh_skills = retained_skills.is_none();
    let (skills, skill_warnings) = retained_skills
        .map(|skills| (skills, Vec::new()))
        .unwrap_or_else(|| discovery.load_skills(cwd, &skills_settings));
    for warning in &skill_warnings {
        let at = if warning.skill_path.is_empty() {
            String::new()
        } else {
            format!(" ({})", warning.skill_path)
        };
        eprintln!("ara: skill warning{at}: {}", warning.message);
    }

    let projector = retained_tools
        .as_ref()
        .and_then(|retained| retained.projector.clone())
        .unwrap_or_else(|| {
            Arc::new(ara_cli::agent_message_projection::AgentMessageProjector::default())
        });
    let projection_journal = retained_tools
        .as_ref()
        .and_then(|retained| retained.projection_journal.clone())
        .unwrap_or_else(|| Arc::new(tokio::sync::Mutex::new(None)));
    let retained_context = retained_tools
        .as_ref()
        .and_then(|retained| retained.context_controller.clone());
    let context_controller = if let Some(controller) = retained_context {
        controller
    } else {
        let settings = rpc_host_settings::append_context_settings(&ara_home().join("agent"))?;
        let preferences = settings.preferences().map_err(anyhow::Error::msg)?;
        let intent_env = std::env::var("ARA_INTENT_TRACING").ok();
        let no_intent_env = std::env::var("ARA_NO_INTENT").ok();
        let build_options = preferences
            .build_options(&model.id, intent_env.as_deref(), no_intent_env.as_deref())
            .map_err(anyhow::Error::msg)?;
        ara_cli::append_only_context_mode::SessionContextController::new(
            settings,
            model,
            model_metadata,
            build_options,
            &preferences.tool_format,
        )
    };

    let mut enabled: Vec<&str> = args
        .tools
        .as_deref()
        .unwrap_or("read,write,edit,bash,grep,glob,ast_grep,ast_edit,debug,web_search,task")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let supplier_settings =
        rpc_host_settings::default_tool_supplier_settings(cwd, &ara_home().join("agent"))?;
    supplier_settings.select(&mut enabled, args.tools.is_some());
    let full_write_grant = args.tools.is_none() || enabled.contains(&"write");
    let xdev_settings = rpc_host_settings::xdev_settings(cwd, &ara_home().join("agent"))?;
    let peer_available=matches!(args.mode,Mode::Rpc)||overlay.peer_coordination||enabled.contains(&"task");
    if args.tools.is_none()&&enabled.contains(&"task")
        &&rpc_host_settings::TaskSettings::load(cwd,&ara_home().join("agent"))?.async_enabled
        &&!enabled.contains(&"wait"){enabled.push("wait");}
    let peer_messaging=peer_available&&rpc_host_settings::peer_messaging_enabled(cwd,&ara_home().join("agent"),overlay.peer_task_depth,overlay.peer_enable_irc.unwrap_or(true))?;
    // Coordination eligibility cannot revoke unrelated xdev device transport.
    let device_only_write = (xdev_settings.enabled || peer_messaging)
        && args.tools.is_some()
        && enabled.contains(&"read")
        && !enabled.contains(&"write");
    if device_only_write {
        enabled.push("write")
    }
    let edit_settings = rpc_host_settings::edit_mode_settings(&ara_home().join("agent"))?;
    let mut read_settings = rpc_host_settings::read_settings(cwd, &ara_home().join("agent"))?;
    if overlay.child_read_summarize==Some(false) {read_settings.summary.enabled=false;}
    let read_host = overlay.read_host.clone().or_else(|| {
        retained_tools
            .as_ref()
            .and_then(|retained| retained.owner.read_host.clone())
    });
    let read_video = overlay.read_video.clone().or_else(|| {
        retained_tools
            .as_ref()
            .and_then(|retained| retained.owner.read_video.clone())
    });
    let read_snapshot = ara_cli::host_browser::ReadHostSnapshot::for_model(
        model,
        model_metadata,
        enabled.contains(&"inspect_image")
            || overlay
                .tools
                .iter()
                .any(|tool| tool.definition().name == "inspect_image"),
        read_settings.screenshot_dir.clone(),
    );
    let read_video_policy = read_video
        .as_ref()
        .map(|_| ara_cli::host_read_video::HostReadVideoPolicy::new(read_snapshot.clone()));
    let read_session = if let Some(host) = &read_host {
        match retained_tools
            .as_ref()
            .and_then(|retained| retained.owner.read_session.as_ref())
        {
            Some(previous)
                if std::fs::canonicalize(cwd).ok().as_deref() == Some(previous.project_dir()) =>
            {
                Some(previous.fork(read_snapshot))
            }
            _ => Some(host.session(cwd, read_snapshot).await?),
        }
    } else {
        None
    };
    let variant = std::env::var("ARA_EDIT_VARIANT").ok();
    let strict = std::env::var("ARA_STRICT_EDIT_MODE").ok();
    let edit_behavior = if enabled.contains(&"edit") {
        let fuzzy = std::env::var("ARA_EDIT_FUZZY").ok();
        let fuzzy_threshold = std::env::var("ARA_EDIT_FUZZY_THRESHOLD").ok();
        edit_mode_policy::resolve_edit_settings(
            &rpc_host_settings::edit_tool_settings(&ara_home().join("agent"))?,
            edit_mode_policy::EditSettingsEnvironment {
                fuzzy: fuzzy.as_deref(),
                fuzzy_threshold: fuzzy_threshold.as_deref(),
            },
        )?
    } else {
        ara_tools::edit::EditSettings::default()
    };
    let fallback_metadata = serde_json::json!({"provider":model.provider,"id":model.id});
    let edit_policy = edit_mode_policy::resolve_edit_mode(
        args.edit_mode,
        &edit_settings,
        edit_mode_policy::EditModeEnvironment {
            variant: variant.as_deref(),
            strict: strict.as_deref(),
        },
        Some(model_metadata.unwrap_or(&fallback_metadata)),
        enabled.contains(&"edit"),
    )?;
    let runtime_policy = live_edit_policy(
        args.edit_mode,
        model_metadata.unwrap_or(&fallback_metadata).clone(),
        enabled.contains(&"edit"),
    );
    let mut lsp_settings=rpc_host_settings::lsp_settings(cwd,&ara_home().join("agent"))?;
    if overlay.child_restrict_lsp {lsp_settings.enabled=false;}
    let lsp_snapshot = ara_cli::host_lsp::LspStartupSnapshot::load(cwd,&ara_home().join("agent"),lsp_settings)?;
    let lsp_startup = retained_tools
        .as_ref()
        .and_then(|retained| retained.owner.lsp_startup.clone())
        .filter(|owner| owner.snapshot == lsp_snapshot)
        .unwrap_or_else(|| ara_cli::host_lsp::HostLspStartup::new(lsp_snapshot));
    let has_ui = !args.print
        && !matches!(args.mode, Mode::Rpc | Mode::Acp)
        && std::io::stdin().is_terminal()
        && (args.repl || args.prompts.is_empty());
    lsp_startup.schedule_warmup(has_ui);
    let vault_enabled = rpc_host_settings::vault_enabled(cwd, &ara_home().join("agent"))?;
    let vault = retained_tools
        .as_ref()
        .and_then(|retained| retained.owner.vault.clone())
        .filter(|owner| owner.enabled() == vault_enabled)
        .unwrap_or_else(|| ara_cli::host_vault::HostVault::new(vault_enabled));
    let (mut tools, tool_owner) = if let Some(retained) = retained_tools {
        rebind_owned_tools(
            retained,
            cwd,
            args.line_numbers,
            read_settings,
            &skills,
            edit_policy,
            edit_behavior,
            runtime_policy.clone(),
            &enabled,
            refresh_skills,
            lsp_startup.clone(),
            vault.clone(),
            read_host.clone(),
            read_session.clone(),
            read_video.clone(),
            read_video_policy.clone(),
        )
    } else {
        let mut supplied = lsp_startup.supplied();
        supplied.vault = Some(vault.clone());
        let edit_session = ara_cli::host_edit_session::HostEditSession::new(home.clone(), supplied);
        let context = new_tool_context(
            cwd,
            args.line_numbers,
            read_settings,
            &skills,
            edit_policy,
            edit_behavior,
            runtime_policy.clone(),
            enabled.contains(&"edit"),
            &edit_session,
            read_session.as_ref(),
            read_video.as_ref(),
            read_video_policy.as_ref(),
        );
        context.set_uri_port(vault.port(None));
        let owned_builtins = owned_builtin_tools(&context, &enabled);
        let mut tools = owned_builtins.clone();
        let owned_mcp = if let Some(config) = mcp_config.clone() {
            ara_mcp::connect_owned(config, cwd, &args.mcp_allow, &TOOL_NAMES)
                .await
                .map_err(anyhow::Error::msg)?
        } else {
            Vec::new()
        };
        tools.extend(
            owned_mcp
                .iter()
                .cloned()
                .map(|tool| tool as Arc<dyn ara_agent::AgentTool>),
        );
        (
            tools,
            CliToolOwner {
                read_host,
                read_session,
                read_video,
                read_video_policy,
                lsp_startup: Some(lsp_startup),
                vault: Some(vault),
                uri_base: None,
            uri_peer:None,
                uri_artifacts: None,
                edit_session: Some(edit_session),
                context: Some(context),
                owned_builtins,
                owned_mcp,
                native: None,
                edit_policy,
            },
        )
    };
    for tool in &overlay.tools {
        let name = &tool.definition().name;
        if tools
            .iter()
            .any(|existing| existing.definition().name == *name)
        {
            anyhow::bail!("RPC host tool \"{name}\" conflicts with an existing tool");
        }
    }
    tools.extend(overlay.tools);

    // One raw metadata projection supplies internal gates, public references
    // and the inline catalog. Native aliases never change logical tool grants.
    let protocol_definitions = tools
        .iter()
        .map(|tool| tool.protocol_definition())
        .collect::<Vec<_>>();
    let tool_name_overrides = protocol_definitions
        .iter()
        .filter_map(|definition| {
            definition
                .custom_wire_name
                .as_ref()
                .map(|wire| (definition.name.clone(), wire.clone()))
        })
        .collect();
    let prompt_tools: Vec<PromptTool> = protocol_definitions
        .iter()
        .map(|definition| {
            let name = definition.name.clone();
            let label = overlay.labels.get(&name).cloned().unwrap_or_else(|| {
                name.get(..1)
                    .map(|first| first.to_uppercase() + &name[1..])
                    .unwrap_or_default()
            });
            PromptTool { name, label }
        })
        .collect();
    // Flags win; otherwise the discovered files (upstream `main.ts`
    // `discoverSystemPromptFile` / `discoverAppendSystemPromptFile`).
    let mut prompt_warnings = Vec::new();
    let discovered = |name: &str| {
        discovery
            .discover_prompt_file(cwd, name)
            .map(|p| p.to_string_lossy().into_owned())
    };
    let system_source = args
        .system_prompt
        .clone()
        .or_else(|| discovered("SYSTEM.md"));
    let append_source = args
        .append_system_prompt
        .clone()
        .or_else(|| discovered("APPEND_SYSTEM.md"));
    let custom_prompt = resolve_prompt_input(
        system_source.as_deref(),
        "system prompt",
        &mut prompt_warnings,
    );
    let append_prompt = resolve_prompt_input(
        append_source.as_deref(),
        "append system prompt",
        &mut prompt_warnings,
    );
    let build_options = context_controller.binding().build_options;
    let owned_dialect = match context_controller.tool_format().as_str() {
        "native" => false,
        "auto" => {
            model_metadata.and_then(|value| value.get("supportsTools"))
                == Some(&serde_json::Value::Bool(false))
        }
        _ => true,
    };
    let tool_inventory = if build_options.prune_tool_descriptions == Some(true) || owned_dialect {
        let definitions = protocol_definitions
            .iter()
            .cloned()
            .map(|mut definition| {
                if let Some(wire_name) = &definition.custom_wire_name {
                    definition.name = wire_name.clone();
                }
                definition
            })
            .collect::<Vec<_>>();
        Some(ara_agent::render_tool_inventory(&definitions).map_err(anyhow::Error::new)?)
    } else {
        None
    };
    let options = SystemPromptOptions {
        custom_prompt,
        append_prompt,
        tools: Some(prompt_tools),
        tool_name_overrides,
        tool_inventory,
        intent_field: build_options.intent_tracing.then(|| "i".into()),
        skills: Some(skills),
        model: Some(model.id.clone()),
        urls: InternalUrls {
            agent: peer_messaging,
            skill: enabled.contains(&"read"),
            ..InternalUrls::default()
        },
        has_obsidian: tool_owner
            .vault
            .as_ref()
            .is_some_and(|vault| vault.has_obsidian()),
        ..SystemPromptOptions::default()
    };
    let built =
        build_system_prompt(&discovery, cwd, &options).context("building the system prompt")?;
    for warning in prompt_warnings.iter().chain(&built.warnings) {
        eprintln!("ara: warning: {warning}");
    }
    let mut system_prompt = built.blocks;
    if peer_available{
        system_prompt.push(ara_cli::host_peers::history_uri::HistoryIndexPort::prompt_doc().into());
        if !peer_messaging{system_prompt.push("Peer messaging is disabled for this Session.".into());}
    }
    let hooks: Arc<dyn LoopHooks> = Arc::new(CliHooks {
        peer_ingress: None,
        edit_session: tool_owner.edit_session.clone(),
        edit_native_session: None,
        service_session: None,
        child_contract:None,
        child_yield:None,
        reminder: DateCwdReminder::new(),
        pending_resolution: tool_owner
            .context
            .as_ref()
            .map(|ctx| ctx.pending_resolution.clone())
            .unwrap_or_default(),
        actual_reminder: ara_cli::actual_host::GraphDateCwdReminder::default(),
        native_tool_hooks: None,
        bind_native_tool_context: !matches!(args.mode, Mode::Rpc | Mode::Acp),
        cwd: cwd.to_string_lossy().replace('\\', "/"),
        context_controller: context_controller.clone(),
        model: model.clone(),
        projector: projector.clone(),
        projection_journal: projection_journal.clone(),
    });

    if let Some(context) = &tool_owner.context {
        context.set_device_only_write(device_only_write);
        // Fixed createTools clears the preview only for a requested full WRITE grant.
        if full_write_grant {
            context.set_pending_full_write_description(false);
        }
    }

    Ok(CliSetup {
        explicit_tools: args.tools.is_some(),
        xdev_prompt: None,
        system_prompt,
        tools,
        hooks,
        skills: options.skills.unwrap_or_default(),
        tool_owner,
        context_controller,
        projector,
        projection_journal,
        native_publication: None,
        bind_native_tool_context: !matches!(args.mode, Mode::Rpc | Mode::Acp),
    })
}

// Clone the transport settings, but allocate native provider state per logical
// Session. An unchanged reload deliberately keeps its existing provider.
struct ProviderFactory {
    agent_context: Option<Arc<rpc_host::HostAgentContext>>,
    client: reqwest::Client,
    route: ara_cli::model_route::PreparedRoute,
    account_auth: Option<Arc<ara_cli::auth_storage::AuthStorage>>,
    shared_account: Option<Arc<ara_cli::auth_storage::AuthStorage>>,
    reset_controller: Option<Arc<ara_cli::codex_reset_controller::CodexResetController>>,
    reset_host: Option<Arc<ReferenceResetHost>>,
    // Prepared same-Session candidates borrow this owner until publication.
    // Dropping a failed candidate must not close the currently admitted Host.
    closes_reset_host: bool,
    registry: Option<ara_cli::model_registry::ModelRegistry>,
    metadata: Option<serde_json::Value>,
    session_id: Option<String>,
    logical_session_id: Option<String>,
    quota_recovery: Arc<session_quota_recovery::SessionQuotaRecovery>,
}

struct ProviderLifecycle {
    agent_context: Option<Arc<rpc_host::HostAgentContext>>,
    shared_account: Option<Arc<ara_cli::auth_storage::AuthStorage>>,
    reset_controller: Option<Arc<ara_cli::codex_reset_controller::CodexResetController>>,
    reset_host: Option<Arc<ReferenceResetHost>>,
    quota_recovery: Arc<session_quota_recovery::SessionQuotaRecovery>,
    closes_reset_host: bool,
    generation: u64,
    logical_session_id: Option<String>,
}

impl ProviderFactory {
    fn reset_context(
        &self,
        model: &Model,
        session_id: Option<String>,
    ) -> ara_cli::codex_reset_controller::CodexResetContext {
        let mut context = ara_cli::codex_reset_controller::CodexResetContext::for_model(
            model,
            session_id,
            Some(usage_base_urls(model, self.registry.clone())),
        );
        if let Some(agent) = &self.agent_context { context = context.with_session_scope(agent.auth_scope.clone()); }
        context.host_epoch = self
            .reset_host
            .as_ref()
            .map(|host| host.current.lock().unwrap().epoch);
        context
    }

    fn admit_reset_session(
        &self,
        model: &Model,
        logical_session: Option<String>,
        bound_session: Option<String>,
    ) {
        if let Some(host) = &self.reset_host {
            host.admit(model, logical_session, bound_session);
        }
    }

    fn bind_codex_session(
        &self,
        session_id: String,
        logical_session: Option<String>,
    ) -> Arc<dyn ModelProvider> {
        let logical_session = logical_session.or_else(|| Some(session_id.clone()));
        self.admit_reset_session(
            self.route.model(),
            logical_session.clone(),
            Some(session_id.clone()),
        );
        let observer = self.quota_observer(Some(session_id.clone()), logical_session);
        self.route
            .bind_codex_session_with_observer(self.client.clone(), session_id, observer)
    }

    fn quota_observer(
        &self,
        session_id: Option<String>,
        logical_session: Option<String>,
    ) -> Option<Arc<dyn ara_cli::model_route::RequestReceiptObserver>> {
        let account = self.account_auth.as_ref()?.clone();
        let host = self.reset_host.as_ref()?.clone();
        let model = self.route.model().clone();
        Some(
            self.quota_recovery
                .observer(session_quota_recovery::QuotaBinding {
                    context: ara_cli::auth_storage::AuthRequestContext {
                        session_id: session_id.clone(),
                        model_id: Some(model.id.clone()),
                        base_url: Some(model.base_url.clone()),
                        session_scope: self.agent_context.as_ref().map(|agent| agent.auth_scope.clone()),
                        ..Default::default()
                    },
                    reset_context: self.reset_context(&model, session_id.clone()),
                    resolver: self.route.auth_for_session(session_id.as_deref()),
                    model,
                    generation: self.route.generation(),
                    logical_session,
                    account,
                    host,
                }),
        )
    }

    fn recorded_session_quota(
        &self,
        message: &ara_ai::AssistantMessage,
    ) -> Option<session_quota_recovery::ObservedQuota> {
        self.quota_recovery.recorded(message)
    }

    fn recorded_session_quota_actual(
        &self,
        message: &ara_agent::AgentMessage,
        values: &ara_agent::SessionValues,
    ) -> std::result::Result<
        Option<session_quota_recovery::ObservedQuota>,
        ara_agent::ActualProviderError,
    > {
        self.quota_recovery.recorded_actual(message, values)
    }

    async fn recover_session_quota(
        &self,
        observed: session_quota_recovery::ObservedQuota,
        model: &Model,
        session_id: &str,
        allow_recovery: bool,
        cancel: &CancellationToken,
    ) -> Result<session_quota_recovery::SessionQuotaOutcome> {
        session_quota_recovery::recover(observed, model, session_id, allow_recovery, cancel).await
    }

    fn set_quota_admission(&self, admission: Arc<dyn Fn() -> bool + Send + Sync>) {
        if let Some(host) = &self.reset_host {
            *host.admission.lock().unwrap() = Some(admission);
        }
    }

    async fn settle_quota_feedback(&self) {
        self.quota_recovery.settle().await;
    }

    fn refresh_usage_if_stale(&self, model: &Model, session_id: Option<String>) {
        if let Some(controller) = &self.reset_controller {
            controller.refresh_if_stale(self.reset_context(model, session_id));
        }
    }
    fn context_for(
        &self,
        journal: &SessionJournal,
        projector: &ara_cli::agent_message_projection::AgentMessageProjector,
    ) -> Result<Vec<AgentMessage>> {
        let settings = rpc_host_settings::AutoCompactionPolicy::load(&ara_home().join("agent"))?
            .recovery_settings()?;
        projector.set_donor(
            self.route.model(),
            self.metadata.as_ref(),
            &settings.remote,
            self.route.remote_supports_images(),
        );
        ara_cli::remote_compaction::route_raw_context(
            journal,
            projector,
            self.route.model(),
            self.metadata.as_ref(),
            &settings.remote,
            self.route.remote_supports_images(),
        )
    }

    fn adopt_context(
        &self,
        journal: &SessionJournal,
        context: &mut Vec<AgentMessage>,
        sink: &HostSink,
    ) -> Result<()> {
        match self.context_for(journal, &sink.projector) {
            Ok(messages) => {
                *context = sink.projector.reconcile_owned_context(context, messages)?;
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
        selection: ara_cli::daily_model_config::DailySelection,
        session_id: Option<String>,
        cwd: &Path,
        cancel: &CancellationToken,
        shared_account: Option<Arc<ara_cli::auth_storage::AuthStorage>>,
        registry: Option<ara_cli::model_registry::ModelRegistry>,
    ) -> Result<Self> {
        Self::daily_scoped(client, selection, session_id, cwd, cancel, shared_account, registry, None).await
    }

    async fn daily_scoped(
        client: reqwest::Client,
        selection: ara_cli::daily_model_config::DailySelection,
        session_id: Option<String>,
        cwd: &Path,
        cancel: &CancellationToken,
        shared_account: Option<Arc<ara_cli::auth_storage::AuthStorage>>,
        registry: Option<ara_cli::model_registry::ModelRegistry>,
        agent_context: Option<Arc<rpc_host::HostAgentContext>>,
    ) -> Result<Self> {
        let reset_host = shared_account.as_ref().map(|_| {
            Arc::new(ReferenceResetHost::new(
                ara_home().join("agent"),
                &selection.model,
                session_id.clone(),
            ))
        });
        let reset_controller = shared_account
            .as_ref()
            .map(|account| -> Result<_> {
                let receipts = Arc::new(
                    ara_cli::codex_reset_receipts::SqliteResetReceiptStore::open(
                        ara_home().join("agent/codex-reset-operations.db"),
                    )?,
                );
                Ok(ara_cli::codex_reset_controller::CodexResetController::new(
                    account.clone(),
                    receipts,
                    reset_host.as_ref().expect("reset Host owner").clone(),
                    ara_cli::codex_reset_controller::CodexResetCoordinator::process_shared(),
                ))
            })
            .transpose()?;
        let lifecycle = ProviderLifecycle {
            agent_context,
            shared_account,
            reset_controller,
            reset_host,
            quota_recovery: Arc::new(Default::default()),
            closes_reset_host: true,
            generation: 0,
            logical_session_id: session_id.clone(),
        };
        Self::daily_with_lifecycle(
            client, selection, session_id, cwd, cancel, registry, lifecycle, true,
        )
        .await
    }

    /// Capture current owners for an independent side task. This borrow never
    /// acquires lifecycle-close ownership or admits another main route.
    /// Main CLI request reporting belongs to the main provider drain.
    fn borrowed_side_factory(&self) -> Self {
        Self {
            agent_context: self.agent_context.clone(),
            client: self.client.clone(),
            route: self.route.without_request_text_observer(),
            account_auth: self.account_auth.clone(),
            shared_account: self.shared_account.clone(),
            reset_controller: self.reset_controller.clone(),
            reset_host: self.reset_host.clone(),
            closes_reset_host: false,
            registry: self.registry.clone(),
            metadata: self.metadata.clone(),
            session_id: self.session_id.clone(),
            logical_session_id: self.logical_session_id.clone(),
            quota_recovery: self.quota_recovery.clone(),
        }
    }

    /// Only clone the actual route and immutable identity; no account/model work
    /// or admission occurs until the existing Session adoption commit.
    fn prepare_primary_rebind(&self, id: &str, agent_context: Option<Arc<rpc_host::HostAgentContext>>) -> Self {
        let registry = match (&self.registry, &self.shared_account, &agent_context) {
            (Some(registry), Some(account), Some(agent)) => Some(registry.with_session_scope(account.as_ref().clone(), agent.auth_scope.clone())),
            _ => self.registry.clone(),
        };
        Self {
            agent_context,
            client: self.client.clone(), route: self.route.for_primary_session(id),
            account_auth: self.account_auth.clone(), shared_account: self.shared_account.clone(),
            reset_controller: self.reset_controller.clone(), reset_host: self.reset_host.clone(),
            closes_reset_host: false, registry, metadata: self.metadata.clone(),
            session_id: Some(id.into()), logical_session_id: Some(id.into()),
            quota_recovery: self.quota_recovery.clone(),
        }
    }

    fn adopt_primary_rebind(&mut self, mut candidate: Self) {
        candidate.closes_reset_host = self.closes_reset_host;
        self.closes_reset_host = false;
        // Epoch is admitted by the existing bind_codex_session/admit_reset_session
        // commit caller. Its subsequent for_session binds the prepared primary.
        *self = candidate;
    }

    /// Prepare a route using the current account/reset/feedback owners. Nothing
    /// is admitted until the matching Session journal transaction commits.
    async fn prepare_daily_replacement(
        &self,
        selection: ara_cli::daily_model_config::DailySelection,
        session_id: Option<String>,
        logical_session_id: Option<String>,
        cwd: &Path,
        cancel: &CancellationToken,
    ) -> Result<Self> {
        if cancel.is_cancelled() {
            bail!("model replacement preparation cancelled");
        }
        let lifecycle = ProviderLifecycle {
            agent_context: self.agent_context.clone(),
            shared_account: self.shared_account.clone(),
            reset_controller: self.reset_controller.clone(),
            reset_host: self.reset_host.clone(),
            quota_recovery: self.quota_recovery.clone(),
            closes_reset_host: false,
            generation: self.route.generation().wrapping_add(1),
            logical_session_id,
        };
        let prepared = Self::daily_with_lifecycle(
            self.client.clone(),
            selection,
            session_id,
            cwd,
            cancel,
            self.registry.clone(),
            lifecycle,
            true,
        )
        .await?;
        if cancel.is_cancelled() {
            bail!("model replacement preparation cancelled");
        }
        Ok(prepared)
    }

    /// Native explicit selection observes configured auth without executing a
    /// helper or obtaining an account lease. Actual calls resolve credentials.
    async fn prepare_model_selection(
        &self,
        selection: ara_cli::daily_model_config::DailySelection,
        session_id: Option<String>,
        cwd: &Path,
        cancel: &CancellationToken,
    ) -> Result<Self> {
        let lifecycle = ProviderLifecycle {
            agent_context: self.agent_context.clone(),
            shared_account: self.shared_account.clone(),
            reset_controller: self.reset_controller.clone(),
            reset_host: self.reset_host.clone(),
            quota_recovery: self.quota_recovery.clone(),
            closes_reset_host: false,
            generation: self.route.generation().wrapping_add(1),
            logical_session_id: session_id.clone(),
        };
        Self::daily_with_lifecycle(
            self.client.clone(),
            selection,
            session_id,
            cwd,
            cancel,
            self.registry.clone(),
            lifecycle,
            false,
        )
        .await
    }

    /// After durable selection, transfer lifecycle ownership without an await
    /// or fallible preparation between the route and its admission epoch.
    fn adopt_daily_replacement(
        &mut self,
        mut candidate: Self,
        logical_session: Option<String>,
        bound_session: Option<String>,
    ) {
        debug_assert!(match (&self.reset_host, &candidate.reset_host) {
            (Some(current), Some(next)) => Arc::ptr_eq(current, next),
            (None, None) => true,
            _ => false,
        });
        candidate.closes_reset_host = self.closes_reset_host;
        self.closes_reset_host = false;
        candidate.admit_reset_session(
            candidate.route.model(),
            logical_session.clone(),
            bound_session.clone(),
        );
        candidate.logical_session_id = logical_session;
        if let Some(id) = bound_session {
            // Prepared decorators retain the old epoch. A new request binds
            // after admission; already started requests keep their old owner.
            candidate.route = candidate.route.for_session(&id);
            candidate.session_id = Some(id);
        }
        *self = candidate;
    }

    #[allow(clippy::too_many_arguments)]
    async fn daily_with_lifecycle(
        client: reqwest::Client,
        mut selection: ara_cli::daily_model_config::DailySelection,
        session_id: Option<String>,
        cwd: &Path,
        cancel: &CancellationToken,
        registry: Option<ara_cli::model_registry::ModelRegistry>,
        lifecycle: ProviderLifecycle,
        prepare_auth: bool,
    ) -> Result<Self> {
        use ara_cli::daily_model_config::DailyAuthSource;
        use ara_cli::model_route::{
            FixedRequestAuth, HostDefaultRequestAuth, PreparedRoute, ProtocolOptions,
            RequestAuthResolver,
        };
        let ProviderLifecycle {
            agent_context,
            shared_account,
            reset_controller,
            reset_host,
            quota_recovery,
            closes_reset_host,
            generation,
            logical_session_id,
        } = lifecycle;
        // Native SessionStats consumes headers independently of the branch
        // that supplies credentials or participates in authentication retry.
        let usage_headers = shared_account.as_ref().map(|owner| {
            let observer = ara_cli::session_usage_headers::SessionUsageHeaders::new(
                owner.as_ref().clone(),
                session_id.clone(),
                None,
            );
            if let Some(registry) = registry.clone() {
                observer.with_registry(registry)
            } else {
                observer
            }
        });
        let usage_account = shared_account.clone();
        if let ProtocolOptions::CodexResponses(options) = &mut selection.protocol {
            options.session_id = session_id.clone();
        }
        let required_scope = agent_context.as_ref().filter(|agent| agent.auth_scope.oauth_account_pools.contains_key(&selection.model.provider));
        let (auth, account_auth): (Arc<dyn RequestAuthResolver>, _) = if let Some(agent) = required_scope {
            let account = shared_account.clone().context("Scoped OAuth account owner is unavailable")?;
            let auth = account.session_resolver(session_id.clone(), Some(selection.model.base_url.clone()))
                .with_session_scope(agent.auth_scope.clone());
            (auth, Some(account))
        } else { match selection.auth_source {
            DailyAuthSource::Fixed(lease) => {
                if selection.host_default_auth
                    && let Some(account) = shared_account.clone()
                {
                    let auth = HostDefaultRequestAuth::new(
                        account
                            .session_resolver(
                                session_id.clone(),
                                Some(selection.model.base_url.clone()),
                            )
                            .with_environment_lease(&lease),
                        lease,
                    );
                    (Arc::new(auth), Some(account))
                } else {
                    (Arc::new(FixedRequestAuth::new(lease)), None)
                }
            }
            DailyAuthSource::OpenAiCodex => {
                let account = shared_account
                    .clone()
                    .context("OpenAI account owner is unavailable")?;
                (
                    account.session_resolver(
                        session_id.clone(),
                        Some(selection.model.base_url.clone()),
                    ),
                    Some(account),
                )
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
                        .session_resolver(
                            session_id.clone(),
                            Some(selection.model.base_url.clone()),
                        )
                        .with_environment_lease(&spec.base);
                    if spec.codex_account {
                        primary
                    } else {
                        Arc::new(HostDefaultRequestAuth::new(primary, spec.base.clone()))
                    }
                });
                let auth = ara_cli::config_request_auth::ConfigRequestAuth::new(
                    spec,
                    cwd.to_path_buf(),
                    inner,
                );
                if prepare_auth {
                    auth.prepare(cancel).await.map_err(|_| {
                        anyhow::anyhow!("configured authentication command preparation failed")
                    })?;
                }
                (Arc::new(auth), account)
            }
        }};
        let auth = if let Some(controller) = &reset_controller {
            if let Some(agent) = &agent_context {
                controller.decorate_scoped(auth, session_id.clone(), Some(usage_base_urls(&selection.model, registry.clone())), agent.auth_scope.clone())
            } else { controller.decorate(
                auth,
                session_id.clone(),
                Some(usage_base_urls(&selection.model, registry.clone())),
            ) }
        } else {
            auth
        };
        let loop_guard_policy = configured_loop_guard_policy(selection.loop_guard_policy)?;
        let mut route = PreparedRoute::new(selection.model, selection.protocol, auth, generation)
            .map_err(|error| anyhow::anyhow!("preparing configured model route: {error:?}"))?
            .with_loop_guard_policy(loop_guard_policy);
        if let Some(owner) = usage_headers {
            route = route.with_usage_headers(owner);
        }
        Ok(Self {
            agent_context,
            client,
            route,
            account_auth,
            shared_account: usage_account,
            reset_controller,
            reset_host,
            closes_reset_host,
            registry,
            metadata: Some(selection.metadata),
            session_id,
            logical_session_id,
            quota_recovery,
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
        let (auth_headers, ordinary_headers) = options
            .extra_headers
            .into_iter()
            .partition(|(name, _)| is_credential_header(name));
        options.extra_headers = ordinary_headers;
        let auth = Arc::new(FixedRequestAuth::new(
            RequestAuthLease::new(CredentialIdentity::Runtime, options.api_key.take())
                .with_headers(auth_headers),
        ));
        let protocol = match api {
            Api::AnthropicMessages => {
                ProtocolOptions::Anthropic(ara_ai::providers::anthropic::StreamOptions {
                    strict_tools: anthropic_strict_tools.then_some(true),
                    extra_headers: options.extra_headers,
                    first_event_timeout: options.first_event_timeout,
                    idle_timeout: options.idle_timeout,
                    retry: options.retry,
                    ..Default::default()
                })
            }
            Api::OpenaiCompletions => ProtocolOptions::Completions(options),
            Api::OpenaiResponses => ProtocolOptions::Responses(ResponsesStreamOptions {
                request: ara_ai::providers::openai_responses::RequestOptions {
                    no_strict: options.no_strict,
                    ..Default::default()
                },
                extra_headers: options.extra_headers,
                first_event_timeout: options.first_event_timeout,
                idle_timeout: options.idle_timeout,
                retry: options.retry,
                stateful_responses: responses_stateful,
                ..Default::default()
            }),
            Api::OpenaiCodexResponses => {
                bail!("OpenAI account routes require the daily account resolver")
            }
            Api::ProxyAuto => bail!("proxy discovery must resolve to a concrete protocol"),
        };
        let route = PreparedRoute::new(model, protocol, auth, 0)
            .map_err(|error| anyhow::anyhow!("preparing startup model route: {error:?}"))?;
        let loop_guard_policy = configured_loop_guard_policy(route.loop_guard_policy())?;
        let route = route.with_loop_guard_policy(loop_guard_policy);
        Ok(Self {
            agent_context: None,
            client,
            route,
            account_auth: None,
            shared_account: None,
            reset_controller: None,
            reset_host: None,
            closes_reset_host: false,
            registry: None,
            metadata: None,
            session_id: None,
            logical_session_id: None,
            quota_recovery: Arc::new(Default::default()),
        })
    }

    fn build(&self) -> Arc<dyn ModelProvider> {
        self.route.bind(
            self.client.clone(),
            self.quota_observer(self.session_id.clone(), self.logical_session_id.clone()),
        )
    }
}

fn usage_base_urls(
    model: &Model,
    registry: Option<ara_cli::model_registry::ModelRegistry>,
) -> ara_cli::auth_storage::CredentialBaseUrlResolver {
    let (provider, url) = (model.provider.clone(), model.base_url.clone());
    Arc::new(move |candidate| {
        if candidate == provider {
            Some(url.clone())
        } else {
            registry
                .as_ref()?
                .get_provider_base_url(&candidate.into())
                .ok()??
                .to_utf8()
                .ok()
        }
    })
}

struct ReferenceResetState {
    epoch: u64,
    provider: String,
    model_id: String,
    session_ids: Vec<Option<String>>,
    closed: bool,
}

struct ReferenceResetHost {
    agent_dir: PathBuf,
    current: std::sync::Mutex<ReferenceResetState>,
    admission: std::sync::Mutex<Option<Arc<dyn Fn() -> bool + Send + Sync>>>,
    quota_epoch_cancel: std::sync::Mutex<Option<(u64, CancellationToken)>>,
}

impl ReferenceResetHost {
    fn new(agent_dir: PathBuf, model: &Model, session: Option<String>) -> Self {
        Self {
            agent_dir,
            current: std::sync::Mutex::new(ReferenceResetState {
                epoch: 1,
                provider: model.provider.clone(),
                model_id: model.id.clone(),
                session_ids: vec![session],
                closed: false,
            }),
            admission: std::sync::Mutex::new(None),
            quota_epoch_cancel: std::sync::Mutex::new(None),
        }
    }

    fn admit(&self, model: &Model, logical_session: Option<String>, bound_session: Option<String>) {
        let mut state = self.current.lock().unwrap();
        if state.closed {
            return;
        }
        state.epoch = state.epoch.wrapping_add(1);
        state.provider = model.provider.clone();
        state.model_id = model.id.clone();
        state.session_ids = vec![logical_session, bound_session];
    }

    fn close(&self) {
        self.current.lock().unwrap().closed = true;
    }

    fn owns_context(&self, context: &ara_cli::codex_reset_controller::CodexResetContext) -> bool {
        let state = self.current.lock().unwrap();
        !state.closed
            && context.host_epoch == Some(state.epoch)
            && context.provider == state.provider
            && context.model_id == state.model_id
            && state
                .session_ids
                .contains(&context.usage.context.session_id)
    }

    fn context_for_request(
        &self,
        template: &ara_cli::codex_reset_controller::CodexResetContext,
    ) -> Option<ara_cli::codex_reset_controller::CodexResetContext> {
        let state = self.current.lock().unwrap();
        if state.closed
            || template.provider != state.provider
            || template.model_id != state.model_id
            || !state
                .session_ids
                .contains(&template.usage.context.session_id)
        {
            return None;
        }
        let mut context = template.clone();
        context.host_epoch = Some(state.epoch);
        Some(context)
    }

    fn freeze_quota_cancellation(
        &self,
        context: &ara_cli::codex_reset_controller::CodexResetContext,
        cancel: &CancellationToken,
    ) -> bool {
        let state = self.current.lock().unwrap();
        if state.closed || context.host_epoch != Some(state.epoch) || cancel.is_cancelled() {
            return false;
        }
        let mut frozen = self.quota_epoch_cancel.lock().unwrap();
        // A queued stop revokes this epoch permanently. Consuming that command
        // cannot re-enable an old owned job still preparing a reset POST.
        if frozen
            .as_ref()
            .is_some_and(|(epoch, token)| *epoch == state.epoch && token.is_cancelled())
        {
            return false;
        }
        *frozen = Some((state.epoch, cancel.clone()));
        true
    }
}

impl Drop for ProviderFactory {
    fn drop(&mut self) {
        if self.closes_reset_host
            && let Some(host) = &self.reset_host
        {
            host.close();
        }
    }
}
#[async_trait]
impl ara_cli::codex_reset_controller::CodexResetHost for ReferenceResetHost {
    fn settings(
        &self,
    ) -> std::result::Result<ara_cli::codex_auto_reset::CodexResetSettings, String> {
        rpc_host_settings::CodexResetPolicy::load(&self.agent_dir)
            .map(|policy| policy.settings())
            .map_err(|error| error.to_string())
    }
    fn persist_mode(
        &self,
        mode: ara_cli::codex_auto_reset::CodexAutoRedeemMode,
    ) -> std::result::Result<(), String> {
        rpc_host_settings::CodexResetPolicy::load(&self.agent_dir)
            .and_then(|mut policy| policy.set_mode(mode))
            .map_err(|error| error.to_string())
    }
    fn notice(&self, level: &str, message: &str) {
        eprintln!("ara: {level}: {}", sanitize_text(message));
    }
    fn epoch(&self) -> Option<u64> {
        Some(self.current.lock().unwrap().epoch)
    }
    fn is_current(&self, context: &ara_cli::codex_reset_controller::CodexResetContext) -> bool {
        let admission = self.admission.lock().unwrap().clone();
        let frozen = self.quota_epoch_cancel.lock().unwrap().clone();
        self.owns_context(context)
            && frozen.is_none_or(|(epoch, cancel)| {
                context.host_epoch != Some(epoch) || !cancel.is_cancelled()
            })
            && admission.is_none_or(|admission| admission())
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

async fn open_codex_auth(
    client: reqwest::Client,
) -> Result<Arc<ara_cli::openai_codex_auth::OpenAiCodexAuth>> {
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
    Ok(Arc::new(
        ara_cli::openai_codex_auth::OpenAiCodexAuth::open(ara_home().join("agent/auth.db"), client)
            .await?,
    ))
}

/// This command reuses the terminal's cancellable prompt owner. It never
/// creates a detached blocking stdin reader. Nonterminal Hosts use callbacks;
/// full piped-manual/TUI login parity remains a separate Host obligation.
struct BrowserPromptOwner {
    driver: Option<Arc<ara_cli::terminal::TerminalDriver>>,
    stop: CancellationToken,
    ready: Arc<tokio::sync::Notify>,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl BrowserPromptOwner {
    fn start(
        cancel: &CancellationToken,
    ) -> (Self, Option<tokio::sync::mpsc::UnboundedReceiver<String>>) {
        let driver = ara_cli::terminal::TerminalDriver::for_stdio(ara_edit::EditMode::Hashline);
        let stop = CancellationToken::new();
        let ready = Arc::new(tokio::sync::Notify::new());
        let mut owner = Self {
            driver,
            stop,
            ready,
            task: None,
        };
        let Some(driver) = owner.driver.clone() else {
            return (owner, None);
        };
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let stop = owner.stop.clone();
        let ready = owner.ready.clone();
        let cancel = cancel.clone();
        owner.task = Some(tokio::spawn(async move {
            tokio::select! { biased;
                _ = stop.cancelled() => return,
                _ = cancel.cancelled() => return,
                _ = ready.notified() => {},
            }
            loop {
                let ticket = Arc::new(ara_cli::terminal::driver::PromptReadTicket::default());
                let reader = driver.clone();
                let reader_ticket = ticket.clone();
                let mut read = tokio::task::spawn_blocking(move || {
                    reader.read_prompt_with_ticket(&reader_ticket)
                });
                let input = tokio::select! { biased;
                    _ = stop.cancelled() => {
                        driver.suspend_prompt(&ticket);
                        let _ = (&mut read).await;
                        break;
                    },
                    _ = cancel.cancelled() => {
                        driver.suspend_prompt(&ticket);
                        let _ = (&mut read).await;
                        break;
                    },
                    input = &mut read => input,
                };
                match input {
                    Ok(Ok(Some(line))) => {
                        if sender.send(line).is_err() {
                            break;
                        }
                    }
                    Ok(Ok(None)) => break,
                    _ => {
                        if !stop.is_cancelled() {
                            cancel.cancel();
                        }
                        break;
                    }
                }
            }
        }));
        (owner, Some(receiver))
    }
    async fn finish(&mut self) {
        self.stop.cancel();
        if let Some(driver) = &self.driver {
            driver.cancel_prompt();
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}
impl Drop for BrowserPromptOwner {
    fn drop(&mut self) {
        self.stop.cancel();
        if let Some(driver) = &self.driver {
            driver.cancel_prompt();
        }
    }
}

/// A bounded platform opener; URLs are passed as arguments, never shell text.
async fn open_login_browser(url: String) {
    #[cfg(windows)]
    let mut command = {
        let mut command = tokio::process::Command::new("rundll32.exe");
        command.arg("url.dll,FileProtocolHandler").arg(&url);
        command.creation_flags(0x0800_0000);
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = tokio::process::Command::new("open");
        command.arg(&url);
        command
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut command = tokio::process::Command::new("xdg-open");
        command.arg(&url);
        command
    };
    #[cfg(not(any(windows, unix)))]
    {
        let _ = url;
        return;
    }
    #[cfg(any(windows, unix))]
    {
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        let opened = tokio::time::timeout(Duration::from_secs(3), command.status()).await;
        if !matches!(opened, Ok(Ok(status)) if status.success()) {
            ara_cli::terminal::report_diagnostic(
                "ara: browser could not be opened; open the displayed URL or paste the redirect URL here",
            );
        }
    }
}

async fn run_auth_command(
    command: AuthCommand,
    project_dir: &Path,
    account_auth: &mut Option<Arc<ara_cli::auth_storage::AuthStorage>>,
    cancel: &CancellationToken,
) -> Result<i32> {
    let provider = match &command {
        AuthCommand::Login { provider, .. } | AuthCommand::Logout { provider } => provider,
        AuthCommand::AuthBroker(args) => {
            return broker_management_host::run(args.clone(), project_dir, account_auth, cancel)
                .await;
        }
    };
    if provider != "openai-codex" {
        bail!("account login currently supports openai-codex; other providers are deferred");
    }
    let mut interrupts = listen_for_interrupts()?;
    let stop = cancel.clone();
    let listener = tokio::spawn(async move {
        if interrupts.recv().await.is_some() {
            stop.cancel();
        }
    });
    let result = async {
        use ara_cli::auth_broker_discover::{
            BrokerConfigResolver, BrokerDiscoveryHost, DiscoverRemoteOptions, client_identity,
            discover_remote_auth_storage,
        };
        // Auth commands need credential discovery, independently of model
        // configuration and Registry preflight (a login may be the first grant).
        let key_resolver = Arc::new(ara_cli::config_request_auth::ConfigStorageKeyResolver::new(
            project_dir.to_path_buf(),
            Arc::new(ara_cli::model_config_values::ConfigValueResolver::new()),
            Arc::new(ara_cli::model_config_values::ProcessConfigEnvironment),
        ));
        let discovery_host = BrokerDiscoveryHost::for_process(ara_home(), Some(key_resolver.clone()));
        let config = BrokerConfigResolver::new(discovery_host.clone()).resolve(cancel).await?;
        let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build()?;
        let options =
            ara_cli::auth_storage::AuthStorageOptions { config_key_resolver: key_resolver, ..Default::default() };
        let account = Arc::new(match config {
            Some(config) => {
                let identity = client_identity(&discovery_host);
                discover_remote_auth_storage(
                    config,
                    DiscoverRemoteOptions {
                        host: discovery_host,
                        client_options: Default::default(),
                        cache_path: None,
                        account_pool: None,
                        identity,
                    },
                    options,
                    cancel,
                )
                .await?
            }
            None => ara_cli::auth_storage::AuthStorage::for_codex(open_codex_auth(client.clone()).await?, options)?,
        });
        // Retain this owner even on a cancelled/uncertain write. run() drains
        // already dispatched mutations and cache writers before returning.
        *account_auth = Some(account.clone());
        match command {
            AuthCommand::AuthBroker(_) => {
                unreachable!("management command dispatched before account discovery")
            }
            AuthCommand::Logout { .. } => {
                account.remove("openai-codex", cancel).await?;
                eprintln!("ara: signed out of OpenAI");
            }
            AuthCommand::Login { method, .. } => {
                #[cfg(feature = "test-fixture")]
                let fixture = std::env::var("ARA_TEST_CODEX_AUTH_BASE_URL").ok();
                let credential = match method {
                    LoginMethod::Device => {
                        #[cfg(feature = "test-fixture")]
                        let issuer = match fixture.as_deref() {
                            Some(base) => {
                                ara_cli::openai_codex_auth::OpenAiCodexDeviceLogin::with_endpoints(client, base)?
                            }
                            None => ara_cli::openai_codex_auth::OpenAiCodexDeviceLogin::new(client),
                        };
                        #[cfg(not(feature = "test-fixture"))]
                        let issuer = ara_cli::openai_codex_auth::OpenAiCodexDeviceLogin::new(client);
                        issuer
                            .issue(cancel, |info| {
                                eprintln!("ara: open {} and enter code {}", info.verification_url, info.user_code);
                            })
                            .await?
                    }
                    LoginMethod::Browser => {
                        #[cfg(feature = "test-fixture")]
                        let issuer = match fixture.as_deref() {
                            Some(base) => {
                                let port = std::env::var("ARA_TEST_CODEX_CALLBACK_PORT")
                                    .ok()
                                    .map(|value| value.parse::<u16>())
                                    .transpose()?
                                    .unwrap_or(1455);
                                let timeout = std::env::var("ARA_TEST_CODEX_CALLBACK_TIMEOUT_MS")
                                    .ok()
                                    .map(|value| value.parse::<u64>())
                                    .transpose()?
                                    .unwrap_or(300_000);
                                ara_cli::openai_codex_auth::OpenAiCodexBrowserLogin::with_endpoints(client, base, port)?
                                    .with_callback_timeout(Duration::from_millis(timeout))
                            }
                            None => ara_cli::openai_codex_auth::OpenAiCodexBrowserLogin::new(client),
                        };
                        #[cfg(not(feature = "test-fixture"))]
                        let issuer = ara_cli::openai_codex_auth::OpenAiCodexBrowserLogin::new(client);
                        #[cfg(feature = "test-fixture")]
                        let open_browser = fixture.is_none();
                        #[cfg(not(feature = "test-fixture"))]
                        let open_browser = true;
                        let (mut prompt, manual) = BrowserPromptOwner::start(cancel);
                        let mut opener = None;
                        let issued = issuer
                            .issue(
                                cancel,
                                manual,
                                |info| {
                                    eprintln!("ara: {}", info.url);
                                    eprintln!("ara: Local shortcut (this machine only): {}", info.launch_url);
                                    eprintln!("ara: {}", info.instructions);
                                    if prompt.driver.is_some() {
                                        eprintln!("ara: paste the authorization code or redirect URL; Ctrl+C cancels");
                                    }
                                    if open_browser {
                                        opener = Some(tokio::spawn(open_login_browser(info.url)));
                                    }
                                    prompt.ready.notify_one();
                                },
                                |progress| {
                                    ara_cli::terminal::report_diagnostic(progress);
                                    if progress == "Exchanging authorization code for tokens..." {
                                        prompt.stop.cancel();
                                        if let Some(driver) = &prompt.driver {
                                            driver.cancel_prompt();
                                        }
                                    }
                                },
                            )
                            .await;
                        prompt.finish().await;
                        if let Some(opener) = opener {
                            let _ = opener.await;
                        }
                        issued?
                    }
                };
                account.upsert_oauth_credential("openai-codex", &credential, cancel).await?;
                eprintln!("ara: signed in to OpenAI; use --provider openai-codex --model <model-id>");
            }
        }
        Ok(0)
    }
    .await;
    listener.abort();
    result
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
    let native_jobs = native_setup::NativeSessionJobs::default();
    let mut read_host = None;
    let mut read_video = None;
    let result = run_inner(
        args,
        &mut account_auth,
        &registry_cancel,
        native_jobs.clone(),
        &mut read_host,
        &mut read_video,
    )
    .await;
    if let Some(owner) = CLI_AUTO_QA.get() {
        let receipts = owner
            .close_and_settle(std::time::Duration::from_millis(6500))
            .await;
        if !receipts.is_empty() {
            eprintln!(
                "ara: autoqa background settlement {}",
                serde_json::to_string(&receipts)?
            );
        }
    }
    let task_result=task_print::close_all().await;
    let job_result = native_setup::close_session_jobs(&native_jobs).await;
    let job_result=match(task_result,job_result){(Ok(()),result)=>result,(Err(error),Ok(()))=>Err(error),(Err(error),Err(other))=>Err(error.context(format!("Session jobs settlement: {other:#}")))};
    let media_result = if let Some(host) = read_video {
        host.close()
            .await
            .map(|()| eprintln!("ara: READ media Host cleanup completed"))
    } else {
        Ok(())
    };
    let job_result = match (job_result, media_result) {
        (result, Ok(())) => result,
        (Ok(()), Err(error)) => Err(error),
        (Err(primary), Err(secondary)) => {
            Err(primary.context(format!("READ media Host settlement: {secondary:#}")))
        }
    };
    let read_result = if let Some(host) = read_host {
        host.close().await.map(|receipts| {
            for receipt in receipts {
                eprintln!(
                    "ara: PDF Host cleanup wrapper={} project={} {}",
                    receipt.wrapper_pid,
                    receipt.project_dir.display(),
                    receipt.cleanup
                );
            }
        })
    } else {
        Ok(())
    };
    let job_result = match (job_result, read_result) {
        (result, Ok(())) => result,
        (Ok(()), Err(error)) => Err(error),
        (Err(primary), Err(secondary)) => {
            Err(primary.context(format!("PDF Host settlement: {secondary:#}")))
        }
    };
    session_quota_recovery::settle_orphaned_feedback().await;
    ara_cli::codex_reset_controller::CodexResetCoordinator::process_shared()
        .close()
        .await;
    registry_cancel.cancel();
    // Normal error/deadline/first-interrupt exits must finish any dispatched
    // refresh settlement before main shuts down the runtime. Hard process
    // termination still cannot prove a remote grant's outcome.
    if let Some(account) = account_auth {
        account.close_and_wait().await;
    }
    ara_cli::config_request_auth::wait_for_config_settlement().await;
    ara_cli::auth_broker_snapshot_cache::wait_for_cache_writes().await;
    match (result, job_result) {
        (result, Ok(())) => result,
        (Ok(_), Err(error)) => Err(error),
        (Err(primary), Err(secondary)) => {
            Err(primary.context(format!("Session Bash settlement: {secondary:#}")))
        }
    }
}

// Keep execution-future construction outside the process teardown poll frame.
fn run_inner<'a>(
    args: Args,
    account_auth: &'a mut Option<Arc<ara_cli::auth_storage::AuthStorage>>,
    registry_cancel: &'a CancellationToken,
    native_jobs: native_setup::NativeSessionJobs,
    read_host: &'a mut Option<Arc<ara_cli::host_browser::HostBrowser>>,
    read_video: &'a mut Option<Arc<ara_cli::host_read_video::HostReadVideo>>,
) -> Pin<Box<impl Future<Output = Result<i32>> + 'a>> {
    Box::pin(run_inner_owned(
        args,
        account_auth,
        registry_cancel,
        native_jobs,
        read_host,
        read_video,
    ))
}

async fn run_inner_owned(
    mut args: Args,
    account_auth: &mut Option<Arc<ara_cli::auth_storage::AuthStorage>>,
    registry_cancel: &CancellationToken,
    native_jobs: native_setup::NativeSessionJobs,
    read_host: &mut Option<Arc<ara_cli::host_browser::HostBrowser>>,
    read_video: &mut Option<Arc<ara_cli::host_read_video::HostReadVideo>>,
) -> Result<i32> {
    if let Some(command) = args.command.take() {
        let project_dir = match &args.cwd {
            Some(path) => plain_drive_path(
                std::fs::canonicalize(path).with_context(|| format!("--cwd {}", path.display()))?,
            ),
            None => std::env::current_dir()?,
        };
        return run_auth_command(command, &project_dir, account_auth, registry_cancel).await;
    }
    let mut selected_thinking = None;
    for value in std::mem::take(&mut args.thinking) {
        match ara_cli::session_thinking::parse_cli(&value) {
            Some(level) => {
                selected_thinking =
                    Some(ara_cli::session_thinking::configured_name(level).to_owned())
            }
            None => {
                eprintln!(
                    "ara: warning: invalid --thinking value {}; expected off, auto, minimal, low, medium, high, xhigh or max",
                    sanitize_text(&value)
                );
            }
        }
    }
    args.thinking = selected_thinking.into_iter().collect();
    let _ = args.print;
    let acp_mode = args.mode == Mode::Acp;
    let rpc_mode = matches!(args.mode, Mode::Rpc | Mode::Acp);
    if rpc_mode && (args.repl || !args.prompts.is_empty() || args.print) {
        bail!(
            "protocol modes read commands from stdin; omit --repl, --print and positional prompts"
        );
    }
    // Proxy auto-discovery explicitly forbids resume. Reject that pure CLI
    // contract before looking for a journal; config-selected APIs are resolved
    // and validated by the route projection below.
    if args.api() == Api::ProxyAuto {
        validate_route_args(&args)?;
    }
    let explicit_cwd = match &args.cwd {
        Some(c) => {
            let canonical =
                std::fs::canonicalize(c).with_context(|| format!("--cwd {}", c.display()))?;
            Some(plain_drive_path(canonical))
        }
        None => None,
    };
    let launch_cwd = explicit_cwd
        .clone()
        .map(Ok)
        .unwrap_or_else(std::env::current_dir)?;
    *read_video = Some(ara_cli::host_read_video::HostReadVideo::from_environment(
        &launch_cwd,
    )?);
    *read_host = ara_cli::host_browser::HostBrowser::from_environment(&launch_cwd).await?;
    let mut cwd = launch_cwd.clone();
    let session_dir = (!args.no_session).then(|| {
        args.session_dir
            .clone()
            .unwrap_or_else(|| default_session_dir(&launch_cwd))
    });
    let resume_path = match (&session_dir, &args.resume, args.continue_session) {
        (Some(_), Some(path), _) => Some(path.clone()),
        (Some(dir), None, true) => Some(
            latest_session(dir)
                .with_context(|| format!("no session to continue in {}", dir.display()))?,
        ),
        _ => None,
    };
    // Read the resumed journal once to establish helper cwd. Blob resolution,
    // recovery and all journal writes remain after route validation below.
    let resumed_journal = resume_path
        .as_ref()
        .map(|path| {
            SessionJournal::open(path)
                .with_context(|| format!("opening session {}", path.display()))
        })
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
                bail!(
                    "session cwd {} no longer exists; pass --cwd to choose where tools run",
                    session_cwd.display()
                )
            }
        }
    }
    let explicit_generation = ara_cli::daily_model_config::DailyGeneration {
        max_tokens: args.max_tokens,
        temperature: args.temperature,
    };
    let mut route = resolve_startup_route(
        &mut args,
        &cwd,
        account_auth,
        registry_cancel,
        resumed_journal.as_ref(),
    )
    .await?;
    if args.mcp_config.is_none() && !args.mcp_allow.is_empty() {
        bail!("--mcp-allow requires --mcp-config");
    }
    let mcp_config = args
        .mcp_config
        .as_deref()
        .map(McpServerConfig::from_file)
        .transpose()
        .map_err(anyhow::Error::msg)?;
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
    let repl_mode =
        !rpc_mode && (args.repl || (prompts.is_empty() && std::io::stdin().is_terminal()));
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
        && args.resume.is_none()
        && !args.continue_session
        && j.current_model()?.as_deref() != Some(model_ref.as_str())
    {
        j.append_model_change(&model_ref)?;
    }
    let header = journal
        .as_ref()
        .map(|j| j.header().clone())
        .unwrap_or_else(|| {
            // One unique logical identity in the sole in-memory journal, also
            // used by project service completion ownership under --no-session.
            ephemeral_header(&cwd, None)
        });
    let session_path = journal.as_ref().map(|j| j.path().to_path_buf());

    let mut setup = prepare_cli_setup(
        &args,
        &cwd,
        &route.model,
        route.daily.as_ref().map(|selection| &selection.metadata),
        &mcp_config,
        None,
        ToolOverlay {
            peer_task_depth: rpc_host_settings::peer_task_depth(journal.as_ref())?,
            read_host: read_host.clone(),
            read_video: read_video.clone(),
            ..ToolOverlay::default()
        },
    )
    .await?;
    if journal.is_none() {
        journal = Some(SessionJournal::in_memory(header.clone())?);
    }
    if let Some(owner) = &setup.tool_owner.read_session {
        owner
            .bind_session(journal.as_ref().expect("Session initialized").session_id())
            .map_err(|error| anyhow::Error::msg(error.0))?;
    }
    if let Some(owner) = &setup.tool_owner.edit_session {
        let journal = journal.as_ref().expect("Session initialized");
        owner.bind(
            journal,
            ara_cli::session_artifacts::SessionArtifacts::for_journal(journal),
        )?;
    }
    let draft = ara_cli::native_factory::PreparedNativeSession::prepare()?;
    let host_options = native_setup_options(&setup, &draft, &[])?;
    let native_publication = native_setup::prepare_cli_native(
        &mut setup,
        draft,
        Vec::new(),
        &cwd,
        &route.model,
        host_options,
    )?;
    native_setup::remember_session_jobs(&native_jobs, &native_publication.native_owner);
    let native_session = native_publication.prepared.clone();
    native_session.publish_catalog_snapshot(native_publication.pending_snapshot.clone(), None)?;
    setup.native_publication = Some(native_publication);
    bind_native_journal(
        &setup.projector,
        journal.as_mut().expect("Session initialized"),
        native_session.values(),
    )?;
    let CliSetup {
        explicit_tools: _,
        xdev_prompt: _,
        system_prompt,
        tools,
        hooks,
        skills,
        mut tool_owner,
        context_controller,
        projector,
        projection_journal,
        native_publication,
        bind_native_tool_context: _,
    } = setup;

    let mut stream_options = route.stream_options;
    let mut request_text_stats = None;
    let request_text_task = if args.report_request_text_tokens {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<PreparedRequestTextObservation>(256);
        let observer = RequestTextObserver::new(tx);
        stream_options.request_text_observer = Some(observer.clone());
        request_text_stats = Some(observer.stats());
        eprintln!(
            "ara: prepared message text only; tool payloads, images and request framing are unmeasured"
        );
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
        if let ara_cli::model_route::ProtocolOptions::Completions(options) = &mut selection.protocol
        {
            options.request_text_observer = stream_options.request_text_observer.take();
        }
        ProviderFactory::daily(
            client,
            selection,
            header
                .get("id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
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
    let mut context = journal
        .as_ref()
        .map(|journal| provider_factory.context_for(journal, &projector))
        .transpose()?
        .unwrap_or_default();
    let mut provider = provider_factory.build();
    if let Some(account) = &provider_factory.account_auth {
        *account_auth = Some(account.clone());
    }
    // Match fixed main.ts: start discovery after Session/Host construction.
    // Catalog refresh remains background work; normal print shutdown drains
    // owned authentication and config helpers without awaiting the catalog.
    if let Some(registry) = &route.registry {
        registry
            .runtime()
            .refresh_in_background(ara_cli::model_manager::ModelRefreshStrategy::OnlineIfUncached);
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
            interrupt_mode: rpc_host_settings::load_interrupt_mode(&ara_home().join("agent"))?,
        };
        let max_time = args.max_time;
        let sessions = rpc_host::SessionFactory {
            peer_messaging_enabled: true,
            agent_context: None,
            dir: session_dir,
            cwd,
            provider: provider_factory,
            args,
            explicit_generation,
            mcp_config,
            tool_owner,
            uri_port: None,
            startup: route.startup.take(),
            context_controller: Some(context_controller),
            actual_values_factory: Some(Arc::new(|| {
                Ok(rpc_host::PreparedActualSession::Native(
                    ara_cli::native_factory::PreparedNativeSession::prepare()?,
                ))
            })),
            initial_native_session: Some(native_session),
            native_jobs: native_jobs.clone(),
            peers: None,
            peer_root: None,
            peer_attachment:None,
            acp_plan: None,
        };
        // The factory owns the transferred Session. Release the startup catalog
        // alias before the RPC host joins its output channel at shutdown.
        drop(native_publication);
        #[cfg(feature="test-fixture")]
        if !acp_mode&&let Some(scenario)=std::env::var_os("ARA_TEST_CHILD_CONTRACT_SCENARIO") {
            return child_host::run_fixture(sessions,journal,&PathBuf::from(scenario)).await;
        }
        #[cfg(feature="test-fixture")]
        if !acp_mode&&let Some(scenario)=std::env::var_os("ARA_TEST_PEER_SCENARIO") {
            return peer_host::run_fixture(config,skills,context,journal,header,max_time,sessions,&PathBuf::from(scenario)).await;
        }
        if acp_mode {
            return acp_host::run(acp_host::NativeSeed {
                config,
                skills,
                messages: context,
                journal,
                header,
                max_time,
                sessions,
            })
            .await;
        }
        return rpc_host::run(config, skills, context, journal, header, max_time, sessions).await;
    }
    // The active CliHooks owns the Session subscription; do not retain the startup clone across /clear.
    drop(context_controller);
    let journal = Some(match journal {
        Some(journal) => journal,
        None => SessionJournal::in_memory(header.clone())?,
    });
    let artifact_router = ara_cli::session_artifacts::ArtifactUriRouter::new(
        ara_cli::session_artifacts::SessionArtifacts::for_journal(
            journal.as_ref().expect("Session initialized"),
        ),
        prepared_host_uri(
            tool_owner.vault.as_ref(),
            tool_owner.uri_base.as_ref(),
            tool_owner.uri_peer.as_ref(),
            None,
        ),
    );
    tool_owner.uri_artifacts = Some(artifact_router.clone());
    if let Some(context) = &tool_owner.context {
        context.set_uri_port(artifact_router.clone());
    }
    *projection_journal.lock().await = journal;
    let sink = Arc::new(
        HostSink::new(
            args.mode,
            repl_mode && args.mode == Mode::Text,
            None,
            cancel.clone(),
            tool_owner.edit_policy.tool_mode,
            artifact_router.clone(),
        )
        .with_terminal_presentation(rpc_host_settings::terminal_presentation_settings(
            &cwd,
            &ara_home().join("agent"),
        )?)
        .with_projection_owner(projector.clone(), projection_journal.clone())
        .with_observed_usage(provider_factory.shared_account.clone())
        .with_edit_session(tool_owner.edit_session.clone())
        .with_wait_session(tool_owner.native.as_ref().map(|owner| owner.wait_session.clone()))
        .with_service_session(tool_owner.native.as_ref().and_then(|owner| owner.service_session.clone())),
    );
    if args.mode == Mode::Json {
        sink.write_line(&header.to_string());
    }
    let mut thinking = host_thinking::CliThinking::initialize(
        &args,
        &provider_factory,
        &sink,
        route.startup.as_ref(),
    )
    .await?;
    host_edit_post_apply::install_cli(
        &args,
        &provider_factory,
        &cwd,
        &tool_owner,
        &route.model,
        &hooks,
        &sink,
    )
    .await;

    task_print::install(&args,&cwd,session_dir.clone(),&provider_factory,&tool_owner,native_jobs.clone(),&mcp_config,&sink).await?;
    let mut interrupts = listen_for_interrupts()?;
    if repl_mode {
        let session = ReplSession {
            dir: session_dir,
            cwd: &cwd,
            model_ref: &model_ref,
            skills: &skills,
            tool_owner,
            native_jobs: native_jobs.clone(),
            provider_factory: &provider_factory,
            mcp_config: &mcp_config,
            artifact_router,
        };
        let result = run_repl_loop(
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
            thinking,
            interrupts,
        )
        .await;
        provider_factory.settle_quota_feedback().await;
        return result;
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
    let mut prompt_queue:std::collections::VecDeque<_>=prompts.into();
    loop {
        let peer_input=sink.task_host().map(|host|host.idle_input()).transpose()?.flatten();
        let prompt=if peer_input.is_some(){"Peer wake from an existing agent.".to_owned()}else if let Some(prompt)=prompt_queue.pop_front(){prompt}else{break};
        thinking
            .prepare_turn(
                &args,
                &provider_factory,
                &cwd,
                prompt.clone(),
                &sink,
                &cancel,
            )
            .await?;
        if cancel.is_cancelled() {
            end = RunEnd::Aborted;
            break;
        }
        let config = AgentConfig {
            model: route.model.clone(),
            provider: thinking.bind(provider.clone()),
            system_prompt: system_prompt.clone(),
            tools: tools.clone(),
            tool_choice: None,
            max_tokens: args.max_tokens,
            temperature: args.temperature,
            deadline: args
                .max_time
                .map(|s| Instant::now() + Duration::from_secs_f64(s.max(0.0))),
            max_model_calls: args.max_model_calls,
            hooks: hooks.clone(),
            interrupt_mode: rpc_host_settings::load_interrupt_mode(&ara_home().join("agent"))?,
        };
        let run_input=match peer_input{Some(input)=>input,None=>sink.projector.new_legacy_message(serde_json::to_value(Message::User(UserMessage::text(prompt)))?,None)?};
        let report = run_host_agent(
            vec![run_input],
            &mut context,
            &config,
            &cancel,
            sink.as_ref(),
            tool_owner.native.as_ref(),
            make_streaming_edit_guard(&tool_owner, Some(&ara_home().join("agent")))?,
        )
        .await?;
        // Provider-native compaction replaces the model view with an opaque
        // replay carrier. Print the actual completed response, not that carrier.
        last_printable_assistant = last_assistant(&context, &sink.projector)?;
        end = report.end;
        if end != RunEnd::Completed {
            break;
        }
        match run_compaction(
            &args,
            &config,
            &provider_factory,
            &mut context,
            &sink,
            &cancel,
            false,
        )
        .await
        {
            Ok(true) => {
                let id = sink
                    .journal
                    .lock()
                    .await
                    .as_ref()
                    .context("Session unavailable")?
                    .session_id()
                    .to_owned();
                provider = provider_factory.bind_codex_session(id, None);
            }
            Ok(false) => {}
            Err(e) => eprintln!("ara: auto-compaction failed ({e:#}); inspect the Session receipt"),
        }
        if sink.persist_failed.load(Ordering::SeqCst) {
            break;
        }
    }
    if let Some(host)=sink.task_host(){host.close().await?;}
    provider_factory.settle_quota_feedback().await;
    drop(provider_factory);
    drop(provider);
    if let Some(task) = request_text_task {
        let _ = task.await;
    }
    let dropped_observations = request_text_stats
        .as_ref()
        .map_or(0, |stats| stats.dropped());
    if dropped_observations > 0 {
        eprintln!("ara: {dropped_observations} prepared text observation(s) were dropped");
    }

    if let Some(lsp) = &tool_owner.lsp_startup {
        if let Err(error) = lsp.shutdown().await {
            eprintln!("ara: LSP shutdown failed: {error}");
        }
    }
    if let Some(path) = &session_path
        && path.exists()
    {
        eprintln!("ara: session {}", path.display());
    }
    let mut code = 0;
    let last = match last_printable_assistant {
        Some(message) => Some(message),
        None => last_assistant(&context, &sink.projector)?,
    };
    match end {
        RunEnd::Completed => {
            if let Some(a) = &last {
                if let Some(e) = &a.error_message {
                    eprintln!("{}", sanitize_text(e));
                }
                if args.mode == Mode::Text {
                    for block in &a.content {
                        match block {
                            ara_cli::actual_host::AssistantDisplayBlock::Text(text) => {
                                sink.write_line(&sanitize_text(text))
                            }
                            ara_cli::actual_host::AssistantDisplayBlock::Thinking(thinking)
                                if args.print_thoughts && !thinking.trim().is_empty() =>
                            {
                                sink.write_line(&sanitize_text(thinking))
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        RunEnd::TerminalToolResult => {}
        RunEnd::PreModelStop => {
            eprintln!("ara: stopped before the model call");
            code = 1;
        }
        RunEnd::Deadline => {
            eprintln!("Deadline exceeded");
            code = 1;
        }
        RunEnd::ModelCallBudget => {
            eprintln!(
                "ara: model call limit reached ({})",
                args.max_model_calls.unwrap_or_default()
            );
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

#[cfg(windows)]
fn prevent_standard_pipe_inheritance() -> std::io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::{
        Foundation::{HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation},
        Storage::FileSystem::{FILE_TYPE_PIPE, GetFileType},
    };

    // Windows spawn inherits all inheritable handles, even when a child uses
    // explicit null stdio. A detached mux must not keep its parent's output
    // pipes open. Protect the process's originals once, before any children;
    // explicit Stdio::inherit still creates its own inheritable duplicates.
    for handle in [
        std::io::stdin().as_raw_handle(),
        std::io::stdout().as_raw_handle(),
        std::io::stderr().as_raw_handle(),
    ] {
        if handle.is_null()
            || handle == INVALID_HANDLE_VALUE
            || unsafe { GetFileType(handle) } != FILE_TYPE_PIPE
        {
            continue;
        }
        if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    ara_cli::schema_policy::initialize_process_schema_policy();
    #[cfg(windows)]
    if let Err(error) = prevent_standard_pipe_inheritance() {
        eprintln!("ara: could not protect standard pipe inheritance: {error}");
        std::process::exit(2);
    }
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "__ara_worker_lsp_mux")
    {
        let code = match ara_cli::host_lsp::mux::run_worker_from_environment().await {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("ara: LSP mux worker failed: {error}");
                2
            }
        };
        std::process::exit(code);
    }
    if std::env::args_os().nth(1).is_some_and(|arg| arg == "__ara_worker_services") {
        let code = match ara_cli::host_services::broker::run_worker_from_environment().await {
            Ok(()) => 0,
            Err(error) => { eprintln!("ara: service broker worker failed: {error:#}"); 2 }
        };
        std::process::exit(code);
    }
    ara_cli::native_logger::time_host::initialize_process_performance();
    ara_cli::catalog_discovery::initialize_catalog_process_startup_tls();
    let args = Args::parse();
    let code = match Box::pin(run(args)).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("ara: {e:#}");
            if e.downcast_ref::<startup_selection::StartupModelResolutionFailure>()
                .is_some()
            {
                1
            } else {
                2
            }
        }
    };
    std::process::exit(code);
}

#[cfg(test)]
#[path = "native_observations/edit_post_apply.rs"]
mod native_edit_post_apply_observations;
#[cfg(test)]
#[path = "native_observations/error.rs"]
mod native_error_observations;
#[cfg(test)]
#[path = "native_observations/host_await.rs"]
mod native_host_await_observations;
#[cfg(test)]
#[path = "native_observations/logger_body_host.rs"]
mod native_logger_body_host_observations;
#[cfg(test)]
#[path = "native_observations/logger_host.rs"]
mod native_logger_host_observations;
#[cfg(test)]
#[path = "native_observations/logger_time_host.rs"]
mod native_logger_time_host_observations;
#[cfg(test)]
#[path = "native_observations/realm.rs"]
mod native_realm_observations;
#[cfg(test)]
#[path = "native_observations/rpc_catalog.rs"]
mod native_rpc_catalog_observations;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn prepared_replacement_transfers_current_session_epoch_and_close_ownership() {
        use ara_cli::daily_model_config::{DailyOverrides, resolve_daily_selection};
        use ara_cli::model_route::{
            AuthResolveError, CredentialIdentity, PreparedRoute, ProtocolOptions, RequestAuthLease,
            RequestAuthResolver,
        };

        struct EpochAuth {
            host: Arc<ReferenceResetHost>,
            rebound: Arc<std::sync::Mutex<Vec<(String, u64)>>>,
        }
        #[async_trait]
        impl RequestAuthResolver for EpochAuth {
            async fn resolve(
                &self,
                _model: &Model,
                _cancel: &CancellationToken,
            ) -> Result<RequestAuthLease, AuthResolveError> {
                Ok(RequestAuthLease::new(CredentialIdentity::Keyless, None))
            }
            fn for_session(&self, id: &str) -> Option<Arc<dyn RequestAuthResolver>> {
                self.rebound
                    .lock()
                    .unwrap()
                    .push((id.into(), self.host.current.lock().unwrap().epoch));
                Some(Arc::new(Self {
                    host: self.host.clone(),
                    rebound: self.rebound.clone(),
                }))
            }
        }
        let temp = tempfile::tempdir().unwrap();
        let selection = |id: &str| {
            resolve_daily_selection(
                None,
                &DailyOverrides {
                    provider: Some("fixture".into()),
                    model: Some(id.into()),
                    api: Some("openai-completions".into()),
                    base_url: Some("http://fixture.test/v1".into()),
                    api_key_env: Some("FIXTURE_KEY".into()),
                    ..Default::default()
                },
                &|name: &str| (name == "FIXTURE_KEY").then(|| "fixture-only".into()),
            )
            .unwrap()
        };
        let first = selection("primary");
        let mut factory = ProviderFactory::daily(
            reqwest::Client::new(),
            first,
            Some("launch-session".into()),
            temp.path(),
            &CancellationToken::new(),
            None,
            None,
        )
        .await
        .unwrap();
        let host = Arc::new(ReferenceResetHost::new(
            temp.path().into(),
            factory.route.model(),
            Some("launch-session".into()),
        ));
        factory.reset_host = Some(host.clone());
        let quota_owner = factory.quota_recovery.clone();
        // A logical Session change need not mutate the original launch field.
        factory.admit_reset_session(
            factory.route.model(),
            Some("current-session".into()),
            Some("current-session".into()),
        );
        let old_epoch = host.current.lock().unwrap().epoch;
        let abandoned = factory
            .prepare_daily_replacement(
                selection("abandoned"),
                Some("current-session".into()),
                Some("current-session".into()),
                temp.path(),
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        drop(abandoned);
        assert!(
            !host.current.lock().unwrap().closed,
            "dropping an uncommitted candidate must retain current authority"
        );
        assert_eq!(
            host.current.lock().unwrap().epoch,
            old_epoch,
            "preparation must not admit a new route"
        );
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        assert!(
            factory
                .prepare_daily_replacement(
                    selection("cancelled"),
                    Some("current-session".into()),
                    Some("current-session".into()),
                    temp.path(),
                    &cancelled
                )
                .await
                .is_err()
        );
        assert!(!host.current.lock().unwrap().closed);
        let mut candidate = factory
            .prepare_daily_replacement(
                selection("fallback"),
                Some("bound-request".into()),
                Some("current-session".into()),
                temp.path(),
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(candidate.session_id.as_deref(), Some("bound-request"));
        assert_eq!(
            candidate.logical_session_id.as_deref(),
            Some("current-session")
        );
        assert!(Arc::ptr_eq(&candidate.quota_recovery, &quota_owner));
        assert_eq!(candidate.route.generation(), factory.route.generation() + 1);
        let rebound = Arc::new(std::sync::Mutex::new(Vec::new()));
        candidate.route = PreparedRoute::new(
            candidate.route.model().clone(),
            ProtocolOptions::Completions(Default::default()),
            Arc::new(EpochAuth {
                host: host.clone(),
                rebound: rebound.clone(),
            }),
            candidate.route.generation(),
        )
        .unwrap();
        factory.adopt_daily_replacement(
            candidate,
            Some("current-session".into()),
            Some("bound-request".into()),
        );
        assert_eq!(
            *rebound.lock().unwrap(),
            vec![("bound-request".into(), old_epoch + 1)]
        );
        assert_eq!(
            factory.logical_session_id.as_deref(),
            Some("current-session")
        );
        let state = host.current.lock().unwrap();
        assert!(
            !state.closed,
            "replacing the old factory must not close its transferred owner"
        );
        assert_eq!(state.model_id, "fallback");
        assert_eq!(
            state.session_ids,
            vec![Some("current-session".into()), Some("bound-request".into())]
        );
        drop(state);
        drop(factory);
        assert!(
            host.current.lock().unwrap().closed,
            "the final adopted factory still owns Host shutdown"
        );
    }

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
        let args = json!({ "path": "src/p.rs", "edits": [] })
            .as_object()
            .cloned()
            .unwrap();
        assert_eq!(tool_summary("edit", &args, EditMode::Patch), ": src/p.rs");
    }

    #[test]
    fn sanitize_strips_ansi_and_controls() {
        assert_eq!(
            sanitize_text("a\x1b[31mred\x1b[0m\tb\nc\x07\r"),
            "ared\tb\nc"
        );
        assert_eq!(sanitize_text("x\x1b]52;c;ZXZpbA==\x07y"), "xy");
        assert_eq!(sanitize_text("x\x1b]0;title\x1b\\y\u{9b}z"), "xyz");
        assert_eq!(sanitize_text("中文 ok"), "中文 ok");
    }

    // Exact fixed-upstream write-xdev-dispatch factory cases, through real CLI setup.
    const ORIGINAL_WRITE8_CHILD: &str = "ARA_ORIGINAL_WRITE8_CHILD";
    const ORIGINAL_WRITE8_RECEIPT: &str = "ARA_ORIGINAL_WRITE8_RECEIPT=";

    async fn original_write8_setup(
        cwd: &Path,
        selection: Option<&str>,
        retained: Option<RetainedToolSetup>,
        draft: Option<Arc<ara_cli::native_factory::PreparedNativeSession>>,
        plan: bool,
    ) -> CliSetup {
        let mut argv = vec!["ara", "--print", "--no-skills"];
        if let Some(selection) = selection {
            argv.extend(["--tools", selection]);
        }
        let args = Args::try_parse_from(argv).unwrap();
        let model = Model {
            id: "fixture".into(),
            api: "openai-completions".into(),
            provider: "fixture".into(),
            base_url: "http://127.0.0.1:1".into(),
            reasoning: false,
            max_tokens: None,
            context_window: None,
            tokenizer: None,
        };
        let mut setup = prepare_cli_setup_with_retained_skills(
            &args,
            cwd,
            &model,
            None,
            &None,
            retained,
            ToolOverlay::default(),
            Some(Vec::new()),
        )
        .await
        .unwrap();
        if plan {
            struct OriginalPlan;
            impl ara_cli::host_edit_session::EditPlanPolicy for OriginalPlan {
                fn current_plan_path(&self) -> Option<String> {
                    Some("local://review-plan.md".into())
                }
            }
            // Original getArtifactsDir=temp/artifacts and getSessionId=device-only-plan.
            let journal_path = cwd.join("artifacts.jsonl");
            let header = serde_json::json!({"type":"session","version":ara_session::CURRENT_SESSION_VERSION,
                "id":"device-only-plan","timestamp":"2026-10-10T00:00:00.000Z","cwd":cwd.to_string_lossy()});
            std::fs::write(&journal_path, format!("{header}\n")).unwrap();
            let journal = SessionJournal::open(&journal_path).unwrap();
            let artifacts = ara_cli::session_artifacts::SessionArtifacts::for_journal(&journal);
            assert_eq!(artifacts.directory().unwrap(), cwd.join("artifacts"));
            assert_eq!(journal.session_id(), "device-only-plan");
            let owner = setup.tool_owner.edit_session.as_ref().unwrap();
            owner.bind(&journal, artifacts).unwrap();
            owner
                .bind_protocol_services(None, Arc::new(OriginalPlan))
                .unwrap();
        }
        let draft = draft
            .unwrap_or_else(|| ara_cli::native_factory::PreparedNativeSession::prepare().unwrap());
        let options = native_setup_options(&setup, &draft, &[]).unwrap();
        let publication =
            native_setup::prepare_cli_native(&mut setup, draft, Vec::new(), cwd, &model, options)
                .unwrap();
        publication
            .prepared
            .publish_catalog_snapshot(publication.pending_snapshot.clone(), None)
            .unwrap();
        setup.native_publication = Some(publication);
        setup
    }

    fn original_write8_source(
        setup: &CliSetup,
        name: &str,
    ) -> Option<(usize, ara_rpc::value_heap::ValueHandle)> {
        use ara_rpc::value_heap::ValueSnapshot;
        let prepared = &setup.tool_owner.native.as_ref().unwrap().prepared;
        let values = prepared.values();
        let heap = values.heap();
        let source = prepared
            .require_tool_hooks()
            .unwrap()
            .source(values)
            .unwrap();
        let tools = source.current_tools(values).unwrap().unwrap();
        let ValueSnapshot::Number(length) = heap
            .snapshot(
                &heap
                    .get(&tools, &"length".into(), values.runtime())
                    .unwrap(),
            )
            .unwrap()
        else {
            panic!()
        };
        for index in 0..length as usize {
            let tool = heap
                .get(&tools, &index.to_string().as_str().into(), values.runtime())
                .unwrap();
            let ValueSnapshot::String(tool_name) = heap
                .snapshot(&heap.get(&tool, &"name".into(), values.runtime()).unwrap())
                .unwrap()
            else {
                panic!()
            };
            if tool_name.equals_ascii(name) {
                return Some((index, tool));
            }
        }
        None
    }

    async fn original_write8_execute(
        setup: &CliSetup,
        name: &str,
        id: &str,
        args: serde_json::Value,
    ) -> Result<ara_rpc::value_heap::ValueHandle, ara_agent::ActualProviderError> {
        let prepared = setup.tool_owner.native.as_ref().unwrap().prepared.clone();
        let (index, _) =
            original_write8_source(setup, name).expect("original selected tool is defined");
        let values = prepared.values();
        let heap = values.heap();
        let cancel = CancellationToken::new();
        let driver =
            ara_cli::native_run::NativeRun::begin(prepared.clone(), cancel.clone()).unwrap();
        // Construct the original call ID before capturing its guarded owner slot.
        let hooks = prepared.require_tool_hooks().unwrap();
        let source = hooks.source(values).unwrap();
        let tools = source.current_tools(values).unwrap().unwrap();
        let selected = heap
            .get(&tools, &index.to_string().as_str().into(), values.runtime())
            .unwrap();
        let binding = source.bind_selected(&selected, values).unwrap();
        let args = heap.import_serde(&args).unwrap();
        let call = heap
            .import_serde(&serde_json::json!({"type":"toolCall","id":id,"name":name}))
            .unwrap();
        heap.define_own_data(&call, "arguments".into(), args.clone(), values.runtime())
            .unwrap();
        let mut assistant = serde_json::to_value(ara_ai::AssistantMessage::empty(
            "fixture", "fixture", "fixture",
        ))
        .unwrap();
        assistant["role"] = "assistant".into();
        assistant["stopReason"] = "toolUse".into();
        let root = heap.import_serde(&assistant).unwrap();
        heap.define_own_data(
            &root,
            "content".into(),
            heap.array(vec![call.clone()]).unwrap(),
            values.runtime(),
        )
        .unwrap();
        let owner = ara_agent::AgentMessage::from_graph(root, None).unwrap();
        let tool_context = heap.object().unwrap();
        hooks.bind_run_context(values, &tool_context).unwrap();
        let request = ara_agent::ActualExecuteRequest {
            callable: heap
                .get(&binding.source, &"execute".into(), values.runtime())
                .unwrap(),
            receiver: binding.source,
            arguments_slot: owner.current_arguments_slot(&call).unwrap(),
            owner,
            tool_call: call,
            call_id: id.into(),
            args,
            values: values.clone(),
            cancel,
            on_update: Arc::new(|_| Ok(())),
            tool_context,
        };
        let invoker = binding.capability.actual_invoker().unwrap();
        tokio::time::timeout(
            Duration::from_secs(8),
            driver.drive(invoker.invoke_settled(request)),
        )
        .await
        .unwrap()
        .unwrap()
    }

    fn original_write8_text(setup: &CliSetup, result: &ara_rpc::value_heap::ValueHandle) -> String {
        use ara_rpc::value_heap::ValueSnapshot;
        let values = setup.tool_owner.native.as_ref().unwrap().prepared.values();
        let heap = values.heap();
        let content = heap
            .get(result, &"content".into(), values.runtime())
            .unwrap();
        let ValueSnapshot::Number(length) = heap
            .snapshot(
                &heap
                    .get(&content, &"length".into(), values.runtime())
                    .unwrap(),
            )
            .unwrap()
        else {
            panic!()
        };
        for index in 0..length as usize {
            let entry = heap
                .get(
                    &content,
                    &index.to_string().as_str().into(),
                    values.runtime(),
                )
                .unwrap();
            if let ValueSnapshot::String(kind) = heap
                .snapshot(&heap.get(&entry, &"type".into(), values.runtime()).unwrap())
                .unwrap()
            {
                if kind.equals_ascii("text") {
                    let ValueSnapshot::String(text) = heap
                        .snapshot(&heap.get(&entry, &"text".into(), values.runtime()).unwrap())
                        .unwrap()
                    else {
                        panic!()
                    };
                    return text.to_utf8().unwrap();
                }
            }
        }
        panic!("original result requires text content")
    }
    fn original_write8_is_error(
        setup: &CliSetup,
        result: &ara_rpc::value_heap::ValueHandle,
        expected: bool,
    ) {
        use ara_rpc::value_heap::ValueSnapshot;
        let values = setup.tool_owner.native.as_ref().unwrap().prepared.values();
        let heap = values.heap();
        let value = heap
            .get(result, &"isError".into(), values.runtime())
            .unwrap();
        if expected {
            assert!(matches!(
                heap.snapshot(&value).unwrap(),
                ValueSnapshot::Boolean(true)
            ));
        } else {
            assert!(matches!(
                heap.snapshot(&value).unwrap(),
                ValueSnapshot::Undefined
            ));
        }
    }
    fn original_write8_detail(
        setup: &CliSetup,
        result: &ara_rpc::value_heap::ValueHandle,
        expected: &str,
    ) {
        use ara_rpc::value_heap::ValueSnapshot;
        let values = setup.tool_owner.native.as_ref().unwrap().prepared.values();
        let heap = values.heap();
        let details = heap
            .get(result, &"details".into(), values.runtime())
            .unwrap();
        let xdev = heap
            .get(&details, &"xdev".into(), values.runtime())
            .unwrap();
        let tool = heap.get(&xdev, &"tool".into(), values.runtime()).unwrap();
        let ValueSnapshot::String(tool) = heap.snapshot(&tool).unwrap() else {
            panic!()
        };
        assert!(tool.equals_ascii(expected));
    }
    fn original_write8_rejection(
        setup: &CliSetup,
        error: ara_agent::ActualProviderError,
        expected: &str,
    ) {
        fn message(setup: &CliSetup, error: &ara_agent::ActualProviderError) -> String {
            use ara_rpc::value_heap::{ValueError, ValueSnapshot};
            match error {
                ara_agent::ActualProviderError::Value(ValueError::Thrown(value)) => {
                    let values = setup.tool_owner.native.as_ref().unwrap().prepared.values();
                    let message = values
                        .heap()
                        .get(value, &"message".into(), values.runtime())
                        .unwrap();
                    let ValueSnapshot::String(text) = values.heap().snapshot(&message).unwrap()
                    else {
                        panic!()
                    };
                    text.to_utf8().unwrap()
                }
                ara_agent::ActualProviderError::Compound { primary, .. } => message(setup, primary),
                other => {
                    panic!("original execute must reject with an actual Graph throw, got {other}")
                }
            }
        }
        let actual = message(setup, &error);
        assert!(
            actual.contains(expected),
            "original rejection expected {expected:?}, got {actual:?}"
        );
    }

    // B-7f62475c49: original default factory and original WRITE approval callback.
    async fn original_write_supplier_approval_case(root: &Path) {
        use ara_rpc::value_heap::ValueSnapshot;
        let cwd = root.join("B-7f62475c49");
        std::fs::create_dir_all(&cwd).unwrap();
        let file = cwd.join("target.ts");
        std::fs::write(&file, "legacyWrap(x, value)\n").unwrap();
        let setup = original_write8_setup(&cwd, None, None, None, false).await;
        let (_, write) = original_write8_source(&setup, "write").expect("original default WRITE");
        let values = setup.tool_owner.native.as_ref().unwrap().prepared.values();
        let heap = values.heap();
        let approval = heap
            .get(&write, &"approval".into(), values.runtime())
            .unwrap();
        assert!(matches!(
            heap.snapshot(&approval).unwrap(),
            ValueSnapshot::Function(_)
        ));
        let call = |args: serde_json::Value| {
            heap.call(
                &approval,
                &write,
                &[heap.import_serde(&args).unwrap()],
                values.runtime(),
            )
            .unwrap()
        };
        let tier =
            |path: &str, content: &str| call(serde_json::json!({"path":path,"content":content}));
        let assert_scoped =
            |value: ara_rpc::value_heap::ValueHandle, expected_tier: &str, key: &str| {
                for (field, expected) in [("tier", expected_tier), ("policyKey", key)] {
                    let actual = heap.get(&value, &field.into(), values.runtime()).unwrap();
                    let ValueSnapshot::String(actual) = heap.snapshot(&actual).unwrap() else {
                        panic!("original scoped approval string")
                    };
                    assert!(
                        actual.equals_ascii(expected),
                        "original {field} expected {expected}"
                    );
                }
                assert_eq!(
                    heap.own_enumerable_string_keys(&value, values.runtime())
                        .unwrap()
                        .len(),
                    2
                );
            };
        let assert_exec = |value: ara_rpc::value_heap::ValueHandle| {
            let ValueSnapshot::String(actual) = heap.snapshot(&value).unwrap() else {
                panic!("original literal exec")
            };
            assert!(actual.equals_ascii("exec"));
        };
        let ast_fs_path =
            serde_json::json!({"ops":[{"pat":"legacyWrap($A, $B)","out":"modernWrap($A, $B)"}],"paths":[file]})
                .to_string();
        let ast_internal_path =
            serde_json::json!({"ops":[{"pat":"a","out":"b"}],"paths":["artifact://abc"]})
                .to_string();
        assert_scoped(tier("xd://ast_edit", &ast_fs_path), "write", "ast_edit");
        assert_scoped(
            tier("xd://ast_edit", &ast_internal_path),
            "read",
            "ast_edit",
        );
        assert_scoped(
            tier(
                "xd://debug",
                &serde_json::json!({"action":"sessions"}).to_string(),
            ),
            "read",
            "debug",
        );
        assert_scoped(
            tier(
                "xd://debug",
                &serde_json::json!({"action":"launch","program":"./app"}).to_string(),
            ),
            "exec",
            "debug",
        );
        assert_exec(tier("xd://ast_edit", "{ not json"));
        assert_exec(tier("xd://ast_edit", "[1,2,3]"));
        assert_exec(tier("xd://ast_edit", "\"a string\""));
        assert_exec(tier(
            "xd://ast_edit",
            &serde_json::json!({"paths":[null]}).to_string(),
        ));
        assert_exec(call(serde_json::json!({"path":"xd://ast_edit"})));
        assert_exec(tier("xd://no_such_device", "{}"));
        println!("original-write-extra:pass:B-7f62475c49");
    }

    // B-ed05afa60d: genuine default WebSearch owner, docs and invalid schema route.
    async fn original_write_web_search_case(root: &Path) {
        let cwd = root.join("B-ed05afa60d");
        std::fs::create_dir_all(&cwd).unwrap();
        let setup = original_write8_setup(&cwd, None, None, None, false).await;
        let snapshot = &setup.tool_owner.native.as_ref().unwrap().snapshot;
        assert!(snapshot.devices_enabled());
        assert!(original_write8_source(&setup, "web_search").is_some());
        assert!(
            !snapshot
                .device_names()
                .iter()
                .any(|name| name.equals_ascii("web_search"))
        );
        assert!(original_write8_source(&setup, "write").is_some());
        assert!(original_write8_source(&setup, "read").is_some());
        let docs = original_write8_execute(
            &setup,
            "read",
            "read-xdev-web-search",
            serde_json::json!({"path":"xd://web_search"}),
        )
        .await
        .unwrap();
        assert!(original_write8_text(&setup, &docs).contains("# web_search"));
        let dispatched = original_write8_execute(
            &setup,
            "write",
            "write-xdev-web-search",
            serde_json::json!({"path":"xd://web_search","content":"{}"}),
        )
        .await
        .unwrap();
        original_write8_is_error(&setup, &dispatched, true);
        original_write8_detail(&setup, &dispatched, "web_search");
        assert!(!original_write8_text(&setup, &dispatched).contains("No such tool"));
        println!("original-write-extra:pass:B-ed05afa60d");
    }

    // Supporting actual factory checks: source gates, explicit-list counterpart,
    // and disabled defaults, all through the existing isolated coarse child.
    async fn original_default_supplier_selection(root: &Path) {
        let cwd = root.join("supplier-selection");
        std::fs::create_dir_all(cwd.join(".ara")).unwrap();
        let defaults = original_write8_setup(&cwd, None, None, None, false).await;
        let names = defaults
            .tool_owner
            .native
            .as_ref()
            .unwrap()
            .snapshot
            .device_names();
        assert!(names.iter().any(|name| name.equals_ascii("ast_edit")));
        assert!(names.iter().any(|name| name.equals_ascii("debug")));
        assert!(!names.iter().any(|name| name.equals_ascii("ast_grep")));
        let explicit = original_write8_setup(&cwd, Some("read,edit"), None, None, false).await;
        assert!(original_write8_source(&explicit, "ast_edit").is_some());
        assert!(original_write8_source(&explicit, "debug").is_none());
        assert!(original_write8_source(&explicit, "web_search").is_none());
        std::fs::write(cwd.join(".ara/config.yml"), "astGrep:\n  enabled: true\nastEdit:\n  enabled: false\ndebug:\n  enabled: false\nweb_search:\n  enabled: false\n").unwrap();
        let explicit = original_write8_setup(
            &cwd,
            Some("read,edit,grep,debug,web_search"),
            None,
            None,
            false,
        )
        .await;
        assert!(original_write8_source(&explicit, "ast_grep").is_some());
        for name in ["ast_edit", "debug", "web_search"] {
            assert!(original_write8_source(&explicit, name).is_none());
        }
        println!("write-contract:pass:actual-default-supplier-settings");
    }

    // B-d312591eaa: fixed original factory -> outer WRITE -> same pending AST queue.
    async fn original_write_ast_queue_case(root: &Path) {
        use ara_rpc::value_heap::{ValueKind, ValueSnapshot};
        let cwd = root.join("B-d312591eaa");
        std::fs::create_dir_all(&cwd).unwrap();
        let file = cwd.join("legacy.ts");
        std::fs::write(&file, "legacyWrap(x, value)\n").unwrap();
        let setup = original_write8_setup(&cwd, None, None, None, false).await;
        assert!(original_write8_source(&setup, "read").is_some());
        let (_, write) = original_write8_source(&setup, "write").unwrap();
        assert!(original_write8_source(&setup, "ast_edit").is_none());
        let listing = original_write8_execute(
            &setup,
            "read",
            "read-xd-list",
            serde_json::json!({"path":"xd://"}),
        )
        .await
        .unwrap();
        assert!(original_write8_text(&setup, &listing).contains("xd://ast_edit"));
        let docs = original_write8_execute(
            &setup,
            "read",
            "read-xd-docs",
            serde_json::json!({"path":"xd://ast_edit"}),
        )
        .await
        .unwrap();
        assert!(original_write8_text(&setup, &docs).contains("# ast_edit"));
        // Preserve JSON.stringify insertion order: ops, then paths.
        let content = format!(
            r#"{{"ops":[{{"pat":"legacyWrap($A, $B)","out":"modernWrap($A, $B)"}}],"paths":[{}]}}"#,
            serde_json::to_string(&file.to_string_lossy()).unwrap()
        );
        let args = serde_json::json!({"path":"xd://ast_edit","content":content});
        let values = setup.tool_owner.native.as_ref().unwrap().prepared.values();
        let heap = values.heap();
        let callback = heap
            .get(&write, &"approval".into(), values.runtime())
            .unwrap();
        assert_eq!(heap.kind(&callback).unwrap(), ValueKind::Function);
        let decision = heap
            .call(
                &callback,
                &write,
                &[heap.import_serde(&args).unwrap()],
                values.runtime(),
            )
            .unwrap();
        let declaration = heap
            .stringify(
                &decision,
                ara_rpc::value_heap::StringifyMode::Plain,
                None,
                values.runtime(),
            )
            .unwrap()
            .unwrap()
            .to_utf8()
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&declaration).unwrap(),
            serde_json::json!({"tier":"write","policyKey":"ast_edit"})
        );
        for (key, expected) in [("tier", "write"), ("policyKey", "ast_edit")] {
            let ValueSnapshot::String(actual) = heap
                .snapshot(&heap.get(&decision, &key.into(), values.runtime()).unwrap())
                .unwrap()
            else {
                panic!()
            };
            assert!(actual.equals_ascii(expected));
        }
        let preview = original_write8_execute(&setup, "write", "write-xdev-preview", args)
            .await
            .unwrap();
        original_write8_is_error(&setup, &preview, false);
        original_write8_detail(&setup, &preview, "ast_edit");
        let details = heap
            .get(&preview, &"details".into(), values.runtime())
            .unwrap();
        let xdev = heap
            .get(&details, &"xdev".into(), values.runtime())
            .unwrap();
        for (key, expected) in [("mode", "execute"), ("tier", "write")] {
            let ValueSnapshot::String(actual) = heap
                .snapshot(&heap.get(&xdev, &key.into(), values.runtime()).unwrap())
                .unwrap()
            else {
                panic!()
            };
            assert!(actual.equals_ascii(expected));
        }
        assert!(original_write8_text(&setup, &preview).contains("modernWrap"));
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "legacyWrap(x, value)\n"
        );
        let queue = &setup
            .tool_owner
            .context
            .as_ref()
            .unwrap()
            .pending_resolution;
        let selection = queue
            .native_ast_head()
            .expect("original preview queues its actual native invoker");
        assert_eq!(queue.pending_head().unwrap().1, "ast_edit");
        // Rust public resolve entrance invokes the same queued stage with action=apply.
        let applied = original_write8_execute(
            &setup,
            "write",
            "write-xdev-apply",
            serde_json::json!({"path":"xd://resolve","content":"apply xdev ast edit"}),
        )
        .await
        .unwrap();
        original_write8_is_error(&setup, &applied, false);
        let details = heap
            .get(&applied, &"details".into(), values.runtime())
            .unwrap();
        let xdev = heap
            .get(&details, &"xdev".into(), values.runtime())
            .unwrap();
        let inner = heap.get(&xdev, &"inner".into(), values.runtime()).unwrap();
        for (key, expected) in [("action", "apply"), ("reason", "apply xdev ast edit")] {
            let ValueSnapshot::String(actual) = heap
                .snapshot(&heap.get(&inner, &key.into(), values.runtime()).unwrap())
                .unwrap()
            else {
                panic!()
            };
            assert!(actual.equals_ascii(expected));
        }
        assert!(
            queue.pending_head().is_none(),
            "queued stage {} must finish",
            selection.id
        );
        assert!(
            std::fs::read_to_string(&file)
                .unwrap()
                .contains("modernWrap(x, value)")
        );
        println!("original-write-extra:pass:B-d312591eaa");
    }

    // Fixed OMP596f2da write-xdev-dispatch.test.ts:510–608, complete original docs2.
    // Derived identities are metadata-only; neither clone is registered for execution.
    async fn original_write_docs2_cases(root: &Path) {
        use ara_rpc::{
            WireValue,
            value_heap::{PropertyDescriptor, ValueHandle, ValueSnapshot},
        };
        let mode = |value: &str| WireValue::String(value.into());
        let empty = WireValue::Array(Vec::new());
        for id in ["B-ea2741404b", "B-56afea3b3c"] {
            let temp = tempfile::Builder::new()
                .prefix(if id == "B-ea2741404b" {
                    "write-xdev-docs-"
                } else {
                    "write-xdev-external-"
                })
                .tempdir_in(root)
                .unwrap();
            let cwd = temp.path();
            assert_eq!(
                rpc_host_settings::xdev_settings(cwd, &ara_home().join("agent"))
                    .unwrap()
                    .docs_mode,
                "builtins"
            );
            let setup = original_write8_setup(cwd, None, None, None, false).await;
            let native = setup.tool_owner.native.as_ref().unwrap();
            let values = native.prepared.values();
            let heap = values.heap();
            let runtime = values.runtime();
            let metadata = native
                .snapshot
                .docs_metadata(values, runtime)
                .unwrap()
                .expect("original xdev state");
            let mounted = metadata.list_tools(runtime).unwrap();
            assert!(!mounted.is_empty());
            let text = |value: ValueHandle| {
                let ValueSnapshot::String(value) = heap.snapshot(&value).unwrap() else {
                    panic!("original docs string")
                };
                value.to_utf8().unwrap()
            };
            let first_name = text(heap.get(&mounted[0], &"name".into(), runtime).unwrap());
            let define = |object: &ValueHandle, key: &str, value: ValueHandle| {
                heap.define_own(
                    object,
                    key.into(),
                    PropertyDescriptor::Data {
                        value,
                        writable: false,
                        enumerable: false,
                        configurable: false,
                    },
                    runtime,
                )
                .unwrap();
            };
            let add = |set: &ValueHandle, name: &str| {
                let method = heap.get(set, &"add".into(), runtime).unwrap();
                heap.call(&method, set, &[heap.string(name.into()).unwrap()], runtime)
                    .unwrap();
            };
            let register = |name: &str, source: &ValueHandle| {
                let set = heap.get(&metadata.tools, &"set".into(), runtime).unwrap();
                heap.call(
                    &set,
                    &metadata.tools,
                    &[heap.string(name.into()).unwrap(), source.clone()],
                    runtime,
                )
                .unwrap();
                add(&metadata.mounted_names, name);
            };
            if id == "B-ea2741404b" {
                let giant = heap.object_with_prototype(Some(&mounted[0])).unwrap();
                define(
                    &giant,
                    "name",
                    heap.string("giant_mcp_tool".into()).unwrap(),
                );
                define(
                    &giant,
                    "description",
                    heap.string("x".repeat(10_000 + 1).into()).unwrap(),
                );
                register("giant_mcp_tool", &giant);
                add(&metadata.builtin_names, "giant_mcp_tool");
                let docs = metadata.default_prompt_docs(runtime).unwrap();
                assert!(docs.len() < 48_000 + 10_000);
                let docs = docs.to_utf8().unwrap();
                assert!(docs.contains(&format!("## {first_name}")));
                assert!(docs.contains("## Additional devices (docs on demand)"));
                assert!(docs.contains("- xd://giant_mcp_tool —"));
                assert!(!docs.contains("## giant_mcp_tool"));
            } else {
                let mut original_names = Vec::new();
                let mut iterator = heap.get_iterator(&metadata.mounted_names, runtime).unwrap();
                while let Some(name) = heap.iterator_next(&mut iterator, runtime).unwrap() {
                    original_names.push(name);
                }
                let external = heap.object_with_prototype(Some(&mounted[0])).unwrap();
                define(
                    &external,
                    "name",
                    heap.string("mcp_external_tool".into()).unwrap(),
                );
                define(
                    &external,
                    "description",
                    heap.string(format!("LEDE {} TAIL", "y".repeat(200 * 3)).into())
                        .unwrap(),
                );
                define(
                    &external,
                    "summary",
                    heap.string(format!("SUMMARY {} TAIL", "z".repeat(200 * 3)).into())
                        .unwrap(),
                );
                register("mcp_external_tool", &external);
                let render = |raw_mode: &WireValue, patterns: &WireValue| {
                    metadata
                        .prompt_docs(raw_mode, patterns, runtime)
                        .unwrap()
                        .to_utf8()
                        .unwrap()
                };
                let inline = render(&mode("inline"), &empty);
                assert!(inline.contains("## mcp_external_tool"));
                assert!(inline.contains("LEDE "));
                assert!(!inline.contains("TAIL"));
                assert!(inline.contains("… (full docs: read xd://mcp_external_tool)"));
                let builtins = render(&mode("builtins"), &empty);
                assert!(builtins.contains("## "));
                assert!(!builtins.contains("## mcp_external_tool"));
                assert!(builtins.contains("- xd://mcp_external_tool —"));
                assert!(!builtins.contains("TAIL"));
                let catalog = render(&mode("catalog"), &empty);
                assert!(!catalog.contains(&format!("## {first_name}")));
                assert!(catalog.contains("- xd://"));
                assert!(catalog.contains("- xd://mcp_external_tool —"));
                assert!(
                    metadata
                        .device_docs(&"mcp_external_tool".into(), runtime)
                        .unwrap()
                        .to_utf8()
                        .unwrap()
                        .contains("TAIL")
                );
                let context_mode = heap.object_with_prototype(Some(&mounted[0])).unwrap();
                define(
                    &context_mode,
                    "name",
                    heap.string("mcp__context_mode_ctx_execute".into()).unwrap(),
                );
                let unrelated = heap.object_with_prototype(Some(&mounted[0])).unwrap();
                define(
                    &unrelated,
                    "name",
                    heap.string("mcp__other_server_execute".into()).unwrap(),
                );
                register("mcp__context_mode_ctx_execute", &context_mode);
                register("mcp__other_server_execute", &unrelated);
                let clear = heap
                    .get(&metadata.mounted_names, &"clear".into(), runtime)
                    .unwrap();
                heap.call(&clear, &metadata.mounted_names, &[], runtime)
                    .unwrap();
                for name in original_names {
                    let add = heap
                        .get(&metadata.mounted_names, &"add".into(), runtime)
                        .unwrap();
                    heap.call(&add, &metadata.mounted_names, &[name], runtime)
                        .unwrap();
                }
                add(&metadata.mounted_names, "mcp__context_mode_ctx_execute");
                add(&metadata.mounted_names, "mcp__other_server_execute");
                let allowlist = WireValue::Array(vec![mode("mcp__context_mode_*")]);
                let allowlisted = render(&mode("builtins"), &allowlist);
                assert!(allowlisted.contains("## mcp__context_mode_ctx_execute"));
                assert!(!allowlisted.contains("## mcp__other_server_execute"));
                assert!(allowlisted.contains("- xd://mcp__other_server_execute —"));
                assert!(
                    !render(&mode("catalog"), &allowlist)
                        .contains("## mcp__context_mode_ctx_execute")
                );
                assert!(
                    render(&mode("builtins"), &mode("mcp__context_mode_*"))
                        .contains("- xd://mcp__context_mode_ctx_execute —")
                );
                assert!(
                    render(
                        &mode("builtins"),
                        &WireValue::Array(vec![WireValue::Number(123.0)])
                    )
                    .contains("- xd://mcp__context_mode_ctx_execute —")
                );
            }
            println!("original-write-extra:pass:{id}");
        }
    }

    async fn original_write8_child_if_selected() -> bool {
        if std::env::var_os(ORIGINAL_WRITE8_CHILD).is_none() {
            return false;
        }
        use ara_rpc::value_heap::ValueSnapshot;
        let root = std::env::current_dir().unwrap();
        let ids = [
            "B-88990d0673",
            "B-8912472cf3",
            "B-5f9da364f9",
            "B-9ee3eed411",
            "B-0d5d81cb77",
            "B-9349da3a10",
            "B-3b080c036a",
            "B-2b24f408e0",
        ];
        for (case, id) in ids.iter().enumerate() {
            println!("original-write8:begin:{id}");
            let cwd = root.join(id);
            std::fs::create_dir_all(&cwd).unwrap();
            let selection = match case {
                2 => Some("read,grep"),
                3 | 4 | 6 => Some("read"),
                5 => Some("read,grep,write"),
                7 => Some("grep,glob"),
                _ => None,
            };
            let mut setup = original_write8_setup(&cwd, selection, None, None, case == 4).await;
            match case {
                0 => {
                    assert!(original_write8_source(&setup, "write").is_some());
                    for target in ["xdt://web_search", "xd:/web_search", "xd/web_search"] {
                        let error = original_write8_execute(
                            &setup,
                            "write",
                            &format!("write-{target}"),
                            serde_json::json!({"path":target,"content":"{}"}),
                        )
                        .await
                        .unwrap_err();
                        original_write8_rejection(&setup, error, "Did you mean 'xd://web_search'?");
                    }
                    assert!(!cwd.join("xdt:/web_search").exists());
                    assert!(!cwd.join("xd/web_search").exists());
                    let escaped = original_write8_execute(
                        &setup,
                        "write",
                        "write-explicit-path",
                        serde_json::json!({"path":"./xd/web_search","content":"intentional file"}),
                    )
                    .await
                    .unwrap();
                    original_write8_is_error(&setup, &escaped, false);
                    assert_eq!(
                        std::fs::read_to_string(cwd.join("xd/web_search")).unwrap(),
                        "intentional file"
                    );
                    let error = original_write8_execute(
                        &setup,
                        "write",
                        "write-conflict",
                        serde_json::json!({"path":"conflict://1","content":"x"}),
                    )
                    .await
                    .unwrap_err();
                    original_write8_rejection(&setup, error, "Conflict #1 not found");
                }
                1 => {
                    std::fs::write(cwd.join("haystack.txt"), "alpha\nfallback-needle\nomega\n")
                        .unwrap();
                    assert!(original_write8_source(&setup, "write").is_some());
                    assert!(original_write8_source(&setup, "read").is_some());
                    assert!(
                        !setup
                            .tool_owner
                            .native
                            .as_ref()
                            .unwrap()
                            .snapshot
                            .device_names()
                            .iter()
                            .any(|name| name.equals_ascii("grep"))
                    );
                    let payload = format!(
                        "{{\"pattern\":\"fallback-needle\",\"path\":{}}}",
                        serde_json::to_string(&cwd.to_string_lossy()).unwrap()
                    );
                    let result = original_write8_execute(
                        &setup,
                        "write",
                        "write-xdev-fallback-grep",
                        serde_json::json!({"path":"xd://grep","content":payload}),
                    )
                    .await
                    .unwrap();
                    original_write8_is_error(&setup, &result, false);
                    original_write8_detail(&setup, &result, "grep");
                    assert!(original_write8_text(&setup, &result).contains("fallback-needle"));
                    let docs = original_write8_execute(
                        &setup,
                        "read",
                        "read-xdev-fallback-grep",
                        serde_json::json!({"path":"xd://grep"}),
                    )
                    .await
                    .unwrap();
                    assert!(original_write8_text(&setup, &docs).contains("# grep"));
                    let unknown = original_write8_execute(
                        &setup,
                        "write",
                        "write-xdev-fallback-unknown",
                        serde_json::json!({"path":"xd://no_such_tool","content":"{}"}),
                    )
                    .await
                    .unwrap();
                    original_write8_is_error(&setup, &unknown, true);
                    assert!(
                        original_write8_text(&setup, &unknown)
                            .contains("No such tool: xd://no_such_tool")
                    );
                }
                2 => {
                    assert!(original_write8_source(&setup, "write").is_some());
                    assert!(
                        setup
                            .tool_owner
                            .context
                            .as_ref()
                            .unwrap()
                            .write_transport_policy()
                            .device_only
                    );
                    assert!(
                        setup
                            .tool_owner
                            .native
                            .as_ref()
                            .unwrap()
                            .snapshot
                            .devices_enabled()
                    );
                    let error = original_write8_execute(
                        &setup,
                        "write",
                        "write-device-only-fs",
                        serde_json::json!({"path":cwd.join("nope.txt"),"content":"x"}),
                    )
                    .await
                    .unwrap_err();
                    original_write8_rejection(&setup, error, "Filesystem writes are not available");
                    let unknown = original_write8_execute(
                        &setup,
                        "write",
                        "write-device-only-unknown",
                        serde_json::json!({"path":"xd://no_such_tool","content":"{}"}),
                    )
                    .await
                    .unwrap();
                    original_write8_is_error(&setup, &unknown, true);
                    original_write8_detail(&setup, &unknown, "no_such_tool");
                    assert!(
                        original_write8_text(&setup, &unknown)
                            .contains("No such tool: xd://no_such_tool")
                    );
                }
                3 => {
                    let (_, write) = original_write8_source(&setup, "write").unwrap();
                    let values = setup.tool_owner.native.as_ref().unwrap().prepared.values();
                    let heap = values.heap();
                    let description = || {
                        let ValueSnapshot::String(text) = heap
                            .snapshot(
                                &heap
                                    .get(&write, &"description".into(), values.runtime())
                                    .unwrap(),
                            )
                            .unwrap()
                        else {
                            panic!()
                        };
                        text
                    };
                    let restricted = description();
                    setup
                        .tool_owner
                        .context
                        .as_ref()
                        .unwrap()
                        .set_pending_full_write_description(true);
                    assert_ne!(description(), restricted);
                    assert!(
                        original_write8_source(&setup, "write")
                            .unwrap()
                            .1
                            .same_identity(&write)
                    );
                    let error = original_write8_execute(
                        &setup,
                        "write",
                        "write-pending-full-fs",
                        serde_json::json!({"path":cwd.join("nope.txt"),"content":"x"}),
                    )
                    .await
                    .unwrap_err();
                    original_write8_rejection(&setup, error, "Filesystem writes are not available");
                    assert!(!cwd.join("nope.txt").exists());
                    setup
                        .tool_owner
                        .context
                        .as_ref()
                        .unwrap()
                        .set_pending_full_write_description(false);
                    assert_eq!(description(), restricted);
                }
                4 => {
                    assert!(original_write8_source(&setup, "read").is_some());
                    assert!(original_write8_source(&setup, "write").is_some());
                    let written = original_write8_execute(
                        &setup,
                        "write",
                        "write-device-only-plan-local",
                        serde_json::json!({"path":"local://review-plan.md","content":"plan draft\n"}),
                    )
                    .await
                    .unwrap();
                    original_write8_is_error(&setup, &written, false);
                    let read = original_write8_execute(
                        &setup,
                        "read",
                        "read-device-only-plan-local",
                        serde_json::json!({"path":"local://review-plan.md"}),
                    )
                    .await
                    .unwrap();
                    assert!(original_write8_text(&setup, &read).contains("plan draft"));
                    // Supplemental Native local file selector regression, not another original ID.
                    let raw = original_write8_execute(
                        &setup,
                        "read",
                        "read-device-only-plan-local-raw",
                        serde_json::json!({"path":"local://review-plan.md:raw"}),
                    )
                    .await
                    .unwrap();
                    assert_eq!(original_write8_text(&setup, &raw).trim(), "plan draft");
                    let error = original_write8_execute(
                        &setup,
                        "write",
                        "write-device-only-plan-conflict",
                        serde_json::json!({"path":"conflict://1","content":"@ours"}),
                    )
                    .await
                    .unwrap_err();
                    original_write8_rejection(&setup, error, "Filesystem writes are not available");
                }
                5 => {
                    assert!(
                        !setup
                            .tool_owner
                            .context
                            .as_ref()
                            .unwrap()
                            .write_transport_policy()
                            .device_only
                    );
                    let file = cwd.join("ok.txt");
                    let result = original_write8_execute(
                        &setup,
                        "write",
                        "write-full-grant",
                        serde_json::json!({"path":file,"content":"hello\n"}),
                    )
                    .await
                    .unwrap();
                    original_write8_is_error(&setup, &result, false);
                    assert_eq!(std::fs::read_to_string(file).unwrap(), "hello\n");
                }
                6 => {
                    assert!(
                        setup
                            .tool_owner
                            .context
                            .as_ref()
                            .unwrap()
                            .write_transport_policy()
                            .device_only
                    );
                    let draft = setup.tool_owner.native.as_ref().unwrap().prepared.clone();
                    let session = draft.session_state().clone();
                    let retained = RetainedToolSetup {
                        tools: setup.tools,
                        owner: setup.tool_owner,
                        reset: false,
                        fresh_edit_session: false,
                        plan_write_selection: None,
                        context_controller: Some(setup.context_controller),
                        projector: Some(setup.projector),
                        projection_journal: Some(setup.projection_journal),
                    };
                    setup = original_write8_setup(
                        &cwd,
                        Some("write"),
                        Some(retained),
                        Some(draft.clone()),
                        false,
                    )
                    .await;
                    assert!(Arc::ptr_eq(
                        &draft,
                        &setup.tool_owner.native.as_ref().unwrap().prepared
                    ));
                    assert!(
                        setup
                            .tool_owner
                            .native
                            .as_ref()
                            .unwrap()
                            .prepared
                            .session_state()
                            .same_identity(&session)
                    );
                    assert!(
                        !setup
                            .tool_owner
                            .context
                            .as_ref()
                            .unwrap()
                            .write_transport_policy()
                            .device_only
                    );
                    let file = cwd.join("upgraded.txt");
                    original_write8_execute(
                        &setup,
                        "write",
                        "write-upgraded-grant",
                        serde_json::json!({"path":file,"content":"upgraded\n"}),
                    )
                    .await
                    .unwrap();
                    assert_eq!(std::fs::read_to_string(file).unwrap(), "upgraded\n");
                    // Additional source contract, not a ninth original behavior ID:
                    // tools/index.ts clears pending preview on explicit WRITE and full-set grants.
                    println!("write-contract:begin:pending-preview-cleared-full-grant");
                    for grant in [Some("write"), None] {
                        let draft = setup.tool_owner.native.as_ref().unwrap().prepared.clone();
                        let session = draft.session_state().clone();
                        let retained = RetainedToolSetup {
                            tools: setup.tools,
                            owner: setup.tool_owner,
                            reset: false,
                            fresh_edit_session: false,
                            plan_write_selection: None,
                            context_controller: Some(setup.context_controller),
                            projector: Some(setup.projector),
                            projection_journal: Some(setup.projection_journal),
                        };
                        setup = original_write8_setup(
                            &cwd,
                            Some("read"),
                            Some(retained),
                            Some(draft.clone()),
                            false,
                        )
                        .await;
                        assert!(
                            setup
                                .tool_owner
                                .context
                                .as_ref()
                                .unwrap()
                                .write_transport_policy()
                                .device_only
                        );
                        let (_, write) = original_write8_source(&setup, "write").unwrap();
                        let values = draft.values();
                        let heap = values.heap();
                        let ValueSnapshot::String(restricted) = heap
                            .snapshot(
                                &heap
                                    .get(&write, &"description".into(), values.runtime())
                                    .unwrap(),
                            )
                            .unwrap()
                        else {
                            panic!()
                        };
                        setup
                            .tool_owner
                            .context
                            .as_ref()
                            .unwrap()
                            .set_pending_full_write_description(true);
                        let ValueSnapshot::String(preview) = heap
                            .snapshot(
                                &heap
                                    .get(&write, &"description".into(), values.runtime())
                                    .unwrap(),
                            )
                            .unwrap()
                        else {
                            panic!()
                        };
                        assert_ne!(preview, restricted);
                        let retained = RetainedToolSetup {
                            tools: setup.tools,
                            owner: setup.tool_owner,
                            reset: false,
                            fresh_edit_session: false,
                            plan_write_selection: None,
                            context_controller: Some(setup.context_controller),
                            projector: Some(setup.projector),
                            projection_journal: Some(setup.projection_journal),
                        };
                        setup = original_write8_setup(
                            &cwd,
                            grant,
                            Some(retained),
                            Some(draft.clone()),
                            false,
                        )
                        .await;
                        assert!(Arc::ptr_eq(
                            &draft,
                            &setup.tool_owner.native.as_ref().unwrap().prepared
                        ));
                        assert!(draft.session_state().same_identity(&session));
                        let policy = setup
                            .tool_owner
                            .context
                            .as_ref()
                            .unwrap()
                            .write_transport_policy();
                        assert!(!policy.device_only);
                        assert!(!policy.pending_full_description);
                        let retained = RetainedToolSetup {
                            tools: setup.tools,
                            owner: setup.tool_owner,
                            reset: false,
                            fresh_edit_session: false,
                            plan_write_selection: None,
                            context_controller: Some(setup.context_controller),
                            projector: Some(setup.projector),
                            projection_journal: Some(setup.projection_journal),
                        };
                        setup = original_write8_setup(
                            &cwd,
                            Some("read"),
                            Some(retained),
                            Some(draft.clone()),
                            false,
                        )
                        .await;
                        assert!(Arc::ptr_eq(
                            &draft,
                            &setup.tool_owner.native.as_ref().unwrap().prepared
                        ));
                        let policy = setup
                            .tool_owner
                            .context
                            .as_ref()
                            .unwrap()
                            .write_transport_policy();
                        assert!(policy.device_only);
                        assert!(!policy.pending_full_description);
                        let (_, write) = original_write8_source(&setup, "write").unwrap();
                        let ValueSnapshot::String(restored) = heap
                            .snapshot(
                                &heap
                                    .get(&write, &"description".into(), values.runtime())
                                    .unwrap(),
                            )
                            .unwrap()
                        else {
                            panic!()
                        };
                        assert_eq!(restored, restricted);
                    }
                    println!("write-contract:pass:pending-preview-cleared-full-grant");
                }
                7 => {
                    assert!(original_write8_source(&setup, "write").is_none());
                    assert!(
                        !setup
                            .tool_owner
                            .context
                            .as_ref()
                            .unwrap()
                            .write_transport_policy()
                            .device_only
                    );
                    assert!(
                        !setup
                            .tool_owner
                            .native
                            .as_ref()
                            .unwrap()
                            .snapshot
                            .devices_enabled()
                    );
                }
                _ => unreachable!(),
            }
            println!("original-write8:pass:{id}");
        }
        original_write_ast_queue_case(&root).await;
        original_write_supplier_approval_case(&root).await;
        original_write_web_search_case(&root).await;
        original_write_docs2_cases(&root).await;
        original_default_supplier_selection(&root).await;
        println!(
            "{ORIGINAL_WRITE8_RECEIPT}{}",
            serde_json::json!({"original_ids":ids,"cases":8,"factory":"actual-cli-setup-native-publication"})
        );
        true
    }

    async fn observe_original_write8() {
        let root =
            std::env::temp_dir().join(format!("ara-original-write8-{}", uuid::Uuid::new_v4()));
        let cwd = root.join("cwd");
        let home = root.join("home");
        let agent_home = home.join(".ara");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&agent_home).unwrap();
        let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "tests::journal_failure_cancels_the_run",
                "--exact",
                "--nocapture",
            ])
            .current_dir(&cwd)
            .env(ORIGINAL_WRITE8_CHILD, "1")
            .env("HOME", &home)
            .env("USERPROFILE", &home)
            .env("ARA_HOME", &agent_home)
            .kill_on_drop(true);
        for (key, _) in std::env::vars_os() {
            let name = key.to_string_lossy();
            if name.starts_with("ARA_") && name != ORIGINAL_WRITE8_CHILD && name != "ARA_HOME" {
                command.env_remove(key);
            }
        }
        command.env_remove("PI_DEBUG_STARTUP");
        let output = tokio::time::timeout(Duration::from_secs(60), command.output())
            .await
            .expect("bounded original8 child")
            .unwrap();
        std::fs::write(root.join("stdout.log"), &output.stdout).unwrap();
        std::fs::write(root.join("stderr.log"), &output.stderr).unwrap();
        assert!(
            output.status.success(),
            "original WRITE8 child: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = std::str::from_utf8(&output.stdout).unwrap();
        for id in [
            "B-d312591eaa",
            "B-7f62475c49",
            "B-ed05afa60d",
            "B-ea2741404b",
            "B-56afea3b3c",
        ] {
            assert!(
                stdout
                    .lines()
                    .any(|line| line == format!("original-write-extra:pass:{id}")),
                "original extra receipt {id}"
            );
        }
        let receipt = stdout
            .lines()
            .find_map(|line| line.strip_prefix(ORIGINAL_WRITE8_RECEIPT))
            .expect("original8 receipt");
        let receipt: serde_json::Value = serde_json::from_str(receipt).unwrap();
        assert_eq!(receipt["cases"], 8);
        std::fs::write(
            root.join("receipt.json"),
            serde_json::to_vec_pretty(&receipt).unwrap(),
        )
        .unwrap();
        println!("Original WRITE8 artifacts: {}", root.display());
    }

    #[tokio::test]
    async fn journal_failure_cancels_the_run() {
        if original_write8_child_if_selected().await {
            return;
        }
        if crate::native_logger_time_host_observations::child_if_selected().await {
            return;
        }
        if crate::native_logger_body_host_observations::child_if_selected().await {
            return;
        }
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
        sink.emit(AgentEvent::MessageEnd {
            message: Message::Assistant(a).into(),
        })
        .await;
        assert!(sink.persist_failed.load(Ordering::SeqCst));
        assert!(
            sink.cancel.is_cancelled(),
            "tools of an unrecorded message must not run"
        );
        let _ = std::fs::remove_dir_all(&dir);
        observe_original_write8().await;
        actual_public_boundary_and_legacy_preflight_observations().await;
        actual_builtin_invocation_observations().await;
        crate::native_host_await_observations::observe().await;
        crate::native_edit_post_apply_observations::observe().await;
        crate::native_rpc_catalog_observations::observe().await;
        // Same existing coarse entrance: actual installed globals and error bodies.
        let realm = ara_cli::native_realm::NativeRealm::prepare().unwrap();
        let global_receipts =
            crate::native_realm_observations::observe_native_realm_globals(&realm).unwrap();
        assert!(!global_receipts.is_empty());
        crate::native_logger_host_observations::observe_native_logger_host();
        crate::native_logger_time_host_observations::observe().await;
        crate::native_logger_body_host_observations::observe().await;
        crate::native_realm_observations::observe_native_realm_signal_registration(
            std::env::temp_dir(),
        )
        .unwrap();
        crate::native_realm_observations::observe_native_realm_concurrent_binding(
            std::env::temp_dir(),
        )
        .unwrap();
        let factory = realm.values.factory();
        let classifier = ara_ai::native_error::NativeErrorFunctionOwner::prepare(
            factory,
            ara_ai::native_error::NativeErrorImports {
                globals: realm.globals.environment.clone(),
                extract_retry_hint: None,
            },
        )
        .unwrap();
        let classification_receipts =
            crate::native_error_observations::observe_native_error_classification(
                factory,
                &classifier,
            )
            .unwrap();
        assert!(!classification_receipts.is_empty());
        crate::native_error_observations::observe_native_error_missing_retry_hint(
            factory,
            &classifier,
        )
        .unwrap();
        crate::native_error_observations::observe_native_error_access_order(factory, &classifier)
            .unwrap();
        crate::native_error_observations::observe_native_error_effective_runtime_and_retained_arrow(
            factory,
            &classifier,
        )
        .unwrap();
        let heap = realm.values.heap();
        let ordinary = heap.object().unwrap();
        heap.define_own_data(
            &ordinary,
            "errorStatus".into(),
            heap.number(404.0).unwrap(),
            realm.values.runtime(),
        )
        .unwrap();
        heap.define_own_data(
            &ordinary,
            "errorMessage".into(),
            heap.string("resource missing".into()).unwrap(),
            realm.values.runtime(),
        )
        .unwrap();
        let id = classifier
            .classify_message(factory, &ordinary, realm.values.runtime())
            .unwrap();
        assert!(
            matches!(heap.snapshot(&id).unwrap(), ara_rpc::value_heap::ValueSnapshot::Number(n) if n == 404.0)
        );
        let stored = heap
            .get(&ordinary, &"errorId".into(), realm.values.runtime())
            .unwrap();
        assert!(heap.object_is(&id, &stored).unwrap());
        eprintln!(
            "Native realm/error coarse observations: {} global receipts, {} classification receipts; ordinary 404 exact",
            global_receipts.len(),
            classification_receipts.len()
        );
    }

    // Same existing coarse Host family: real effects and native admission faults.
    async fn actual_builtin_invocation_observations() {
        use ara_agent::{
            ActualExecuteRequest, ActualProviderError, ActualToolExecutionContext,
            ActualToolInvoker, ActualToolSource, SessionValues,
        };
        use ara_cli::actual_tools::{BuiltinKind, BuiltinToolSource, NativeBuiltinToolHooks};
        use ara_rpc::value_heap::{
            CallableTarget, CapabilityId, OpaqueOperation, PropertyDescriptor, PropertyKey,
            RuntimeReply, RuntimeRequest, ValueError, ValueHandle, ValueHeap, ValueRuntime,
            ValueSnapshot,
        };
        use serde_json::json;

        struct BoundaryRuntime {
            heap: Arc<ValueHeap>,
            source: std::sync::Mutex<Option<std::sync::Weak<BuiltinToolSource>>>,
            gets: std::sync::Mutex<Vec<&'static str>>,
            cancel_write: std::sync::Mutex<Option<CancellationToken>>,
        }
        impl ValueRuntime for BoundaryRuntime {
            fn resolve(
                &self,
                request: RuntimeRequest<'_>,
            ) -> std::result::Result<RuntimeReply, ValueError> {
                let source = {
                    self.source
                        .lock()
                        .unwrap()
                        .as_ref()
                        .and_then(std::sync::Weak::upgrade)
                };
                if let Some(reply) =
                    source.and_then(|source| source.resolve_context_signal(&request))
                {
                    return reply;
                }
                match request {
                    RuntimeRequest::Call {
                        capability: CapabilityId(9101),
                        ..
                    } => {
                        self.gets.lock().unwrap().push("path");
                        Ok(RuntimeReply::Value(self.heap.string("denied.txt".into())?))
                    }
                    RuntimeRequest::Call {
                        capability: CapabilityId(9102),
                        ..
                    } => {
                        self.gets.lock().unwrap().push("content");
                        self.source
                            .lock()
                            .unwrap()
                            .as_ref()
                            .unwrap()
                            .upgrade()
                            .unwrap()
                            .set_granted(BuiltinKind::Write, false)
                            .unwrap();
                        Ok(RuntimeReply::Value(
                            self.heap.string("must not be written".into())?,
                        ))
                    }
                    RuntimeRequest::Call {
                        capability: CapabilityId(9103),
                        ..
                    } => {
                        self.gets.lock().unwrap().push("path");
                        Ok(RuntimeReply::Value(
                            self.heap
                                .string("getter-cancel-write/forbidden.txt".into())?,
                        ))
                    }
                    RuntimeRequest::Call {
                        capability: CapabilityId(9104),
                        ..
                    } => {
                        self.gets.lock().unwrap().push("content");
                        self.cancel_write.lock().unwrap().as_ref().unwrap().cancel();
                        Ok(RuntimeReply::Value(
                            self.heap.string("must not be written".into())?,
                        ))
                    }
                    request => Err(ValueError::MissingCapability(request.capability())),
                }
            }
        }
        fn request(
            source: &BuiltinToolSource,
            values: &Arc<SessionValues>,
            index: usize,
            name: &str,
            args: ValueHandle,
            cancel: CancellationToken,
            update: ara_agent::ActualUpdateFn,
        ) -> (Arc<dyn ActualToolInvoker>, ActualExecuteRequest) {
            let heap = values.heap();
            let collection = source.current_tools(values).unwrap().unwrap();
            let selected = heap
                .get(
                    &collection,
                    &ara_rpc::WireString::from(index.to_string()).into(),
                    values.runtime(),
                )
                .unwrap();
            let binding = source.bind_selected(&selected, values).unwrap();
            let tool_call = heap
                .import_serde(&json!({"type":"toolCall","id":"H2","name":name}))
                .unwrap();
            heap.define_own_data(
                &tool_call,
                "arguments".into(),
                args.clone(),
                values.runtime(),
            )
            .unwrap();
            let mut assistant = serde_json::to_value(ara_ai::AssistantMessage::empty(
                "fixture", "fixture", "fixture",
            ))
            .unwrap();
            assistant["role"] = "assistant".into();
            assistant["stopReason"] = "toolUse".into();
            let root = heap.import_serde(&assistant).unwrap();
            heap.define_own_data(
                &root,
                "content".into(),
                heap.array(vec![tool_call.clone()]).unwrap(),
                values.runtime(),
            )
            .unwrap();
            let owner = AgentMessage::from_graph(root, None).unwrap();
            let arguments_slot = owner.current_arguments_slot(&tool_call).unwrap();
            let callable = heap
                .get(&binding.source, &"execute".into(), values.runtime())
                .unwrap();
            let invoker = binding.capability.actual_invoker().unwrap();
            (
                invoker,
                ActualExecuteRequest {
                    callable,
                    receiver: binding.source,
                    owner,
                    tool_call,
                    arguments_slot,
                    call_id: "H2".into(),
                    args,
                    values: values.clone(),
                    cancel,
                    on_update: update,
                    tool_context: heap.object().unwrap(),
                },
            )
        }
        fn contains_throw(error: &ActualProviderError, witness: &ValueHandle) -> bool {
            match error {
                ActualProviderError::Value(ValueError::Thrown(value)) => {
                    value.same_identity(witness)
                }
                ActualProviderError::Compound { primary, secondary } => {
                    contains_throw(primary, witness) || contains_throw(secondary, witness)
                }
                _ => false,
            }
        }
        let dir = std::env::temp_dir().join(format!("ara-native-tools-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let heap = ValueHeap::new().unwrap();
        let runtime = Arc::new(BoundaryRuntime {
            heap: heap.clone(),
            source: Default::default(),
            gets: Default::default(),
            cancel_write: Default::default(),
        });
        let values = Arc::new(SessionValues::new(heap.clone(), runtime.clone()).unwrap());
        // This boundary fixture observes joined foreground effects. Managed
        // background defaults are exercised by the Native Host await group.
        let source = BuiltinToolSource::new_with_bash_session(
            &values,
            ToolContext::new(dir.clone()),
            &[BuiltinKind::Read, BuiltinKind::Write, BuiltinKind::Bash],
            ara_cli::actual_tools::bash_native::SessionBashOwner::new(
                ara_cli::actual_tools::bash_native::BashSessionSettings {
                    auto_background_enabled: false,
                    ..Default::default()
                },
            ),
        )
        .unwrap();
        *runtime.source.lock().unwrap() = Some(Arc::downgrade(&source));
        let noop: ara_agent::ActualUpdateFn = Arc::new(|_| Ok(()));

        // Fixed getToolContext creates fresh attribution, but one execute batch
        // retains the original steering controller's same AbortSignal object.
        let hooks = NativeBuiltinToolHooks::new(source.clone(), None, None, None, &values).unwrap();
        let tool_calls = heap
            .array(vec![heap.object().unwrap(), heap.object().unwrap()])
            .unwrap();
        let provider_metadata = heap.object().unwrap();
        let steering = CancellationToken::new();
        let mut context = ActualToolExecutionContext {
            batch_id: "original signal batch".into(),
            index: 0,
            total: 2,
            tool_calls: tool_calls.clone(),
            provider_metadata: provider_metadata.clone(),
            steering_signal: steering.clone(),
        };
        let first = hooks.tool_context(&context, &values).unwrap();
        context.index = 1;
        let second = hooks.tool_context(&context, &values).unwrap();
        assert!(!first.same_identity(&second));
        let first_call = heap
            .get(&first, &"toolCall".into(), values.runtime())
            .unwrap();
        let second_call = heap
            .get(&second, &"toolCall".into(), values.runtime())
            .unwrap();
        assert!(!first_call.same_identity(&second_call));
        let first_signal = heap
            .get(&first_call, &"steeringSignal".into(), values.runtime())
            .unwrap();
        let second_signal = heap
            .get(&second_call, &"steeringSignal".into(), values.runtime())
            .unwrap();
        assert!(first_signal.same_identity(&second_signal));
        for call in [&first_call, &second_call] {
            assert!(
                heap.get(call, &"toolCalls".into(), values.runtime())
                    .unwrap()
                    .same_identity(&tool_calls)
            );
            assert!(
                heap.get(call, &"providerMetadata".into(), values.runtime())
                    .unwrap()
                    .same_identity(&provider_metadata)
            );
        }
        let aborted = |signal: &ValueHandle| {
            heap.snapshot(
                &heap
                    .get(signal, &"aborted".into(), values.runtime())
                    .unwrap(),
            )
            .unwrap()
        };
        assert!(matches!(
            aborted(&first_signal),
            ValueSnapshot::Boolean(false)
        ));
        assert!(matches!(
            aborted(&second_signal),
            ValueSnapshot::Boolean(false)
        ));

        context.batch_id = "next signal batch".into();
        context.tool_calls = heap.array(vec![heap.object().unwrap()]).unwrap();
        context.index = 0;
        context.total = 1;
        context.steering_signal = CancellationToken::new();
        let next = hooks.tool_context(&context, &values).unwrap();
        let next_call = heap
            .get(&next, &"toolCall".into(), values.runtime())
            .unwrap();
        let next_signal = heap
            .get(&next_call, &"steeringSignal".into(), values.runtime())
            .unwrap();
        assert!(!first_signal.same_identity(&next_signal));
        steering.cancel();
        assert!(matches!(
            aborted(&first_signal),
            ValueSnapshot::Boolean(true)
        ));
        assert!(matches!(
            aborted(&second_signal),
            ValueSnapshot::Boolean(true)
        ));
        assert!(matches!(
            aborted(&next_signal),
            ValueSnapshot::Boolean(false)
        ));

        // Display batch IDs may collide. Fresh infos/controller ownership still
        // creates a new signal, including a Native map reusing the infos owner.
        context.batch_id = "original signal batch".into();
        context.tool_calls = heap.array(vec![heap.object().unwrap()]).unwrap();
        let collision_token = CancellationToken::new();
        context.steering_signal = collision_token.clone();
        let collision = hooks.tool_context(&context, &values).unwrap();
        let collision_call = heap
            .get(&collision, &"toolCall".into(), values.runtime())
            .unwrap();
        let collision_signal = heap
            .get(&collision_call, &"steeringSignal".into(), values.runtime())
            .unwrap();
        assert!(!collision_signal.same_identity(&first_signal));
        assert!(matches!(
            aborted(&collision_signal),
            ValueSnapshot::Boolean(false)
        ));
        let collision_again = hooks.tool_context(&context, &values).unwrap();
        let collision_again_call = heap
            .get(&collision_again, &"toolCall".into(), values.runtime())
            .unwrap();
        assert!(
            heap.get(
                &collision_again_call,
                &"steeringSignal".into(),
                values.runtime()
            )
            .unwrap()
            .same_identity(&collision_signal)
        );

        context.tool_calls = tool_calls.clone();
        context.total = 2;
        let reused_owner_token = CancellationToken::new();
        context.steering_signal = reused_owner_token.clone();
        let reused_owner = hooks.tool_context(&context, &values).unwrap();
        let reused_owner_call = heap
            .get(&reused_owner, &"toolCall".into(), values.runtime())
            .unwrap();
        let reused_owner_signal = heap
            .get(
                &reused_owner_call,
                &"steeringSignal".into(),
                values.runtime(),
            )
            .unwrap();
        assert!(!reused_owner_signal.same_identity(&first_signal));
        assert!(!reused_owner_signal.same_identity(&collision_signal));
        let reused_owner_again = hooks.tool_context(&context, &values).unwrap();
        let reused_owner_again_call = heap
            .get(&reused_owner_again, &"toolCall".into(), values.runtime())
            .unwrap();
        assert!(
            heap.get(
                &reused_owner_again_call,
                &"steeringSignal".into(),
                values.runtime()
            )
            .unwrap()
            .same_identity(&reused_owner_signal)
        );
        collision_token.cancel();
        assert!(matches!(
            aborted(&collision_signal),
            ValueSnapshot::Boolean(true)
        ));
        assert!(matches!(
            aborted(&reused_owner_signal),
            ValueSnapshot::Boolean(false)
        ));
        reused_owner_token.cancel();
        assert!(matches!(
            aborted(&reused_owner_signal),
            ValueSnapshot::Boolean(true)
        ));
        assert!(matches!(
            aborted(&next_signal),
            ValueSnapshot::Boolean(false)
        ));
        context.steering_signal = steering;
        let original_again = hooks.tool_context(&context, &values).unwrap();
        let original_again_call = heap
            .get(&original_again, &"toolCall".into(), values.runtime())
            .unwrap();
        assert!(
            heap.get(
                &original_again_call,
                &"steeringSignal".into(),
                values.runtime()
            )
            .unwrap()
            .same_identity(&first_signal)
        );
        let foreign_session = Arc::new(SessionValues::new(heap.clone(), runtime.clone()).unwrap());
        assert!(matches!(
            hooks.tool_context(&context, &foreign_session),
            Err(ActualProviderError::Protocol(_))
        ));
        let capability = match heap.snapshot(&first_signal).unwrap() {
            ValueSnapshot::Opaque { capability, .. } => capability,
            _ => panic!("Original signal must remain an opaque capability"),
        };
        let wrong_receiver = heap.object().unwrap();
        assert!(matches!(
            runtime.resolve(RuntimeRequest::Opaque {
                capability,
                target: &first_signal,
                receiver: &wrong_receiver,
                operation: OpaqueOperation::Get(&PropertyKey::String("aborted".into())),
            }),
            Err(ValueError::TypeError(_))
        ));
        assert!(matches!(
            heap.get(&first_signal, &"reason".into(), values.runtime()),
            Err(ValueError::UnsupportedOperation(_))
        ));
        assert!(matches!(
            aborted(&first_signal),
            ValueSnapshot::Boolean(true)
        ));

        let args = heap.object().unwrap();
        for (key, id) in [("path", 9101), ("content", 9102)] {
            heap.define_own(
                &args,
                key.into(),
                PropertyDescriptor::Accessor {
                    get: Some(CallableTarget::Host(CapabilityId(id))),
                    set: None,
                    enumerable: true,
                    configurable: true,
                },
                values.runtime(),
            )
            .unwrap();
        }
        let (invoker, call) = request(
            &source,
            &values,
            1,
            "write",
            args,
            CancellationToken::new(),
            noop.clone(),
        );
        let denied = invoker.invoke_settled(call).await.unwrap_err();
        assert!(
            matches!(denied, ActualProviderError::Callback(ref message) if message.contains("current Host grant"))
        );
        assert_eq!(
            *runtime.gets.lock().unwrap(),
            ["path", "content"],
            "fixed binding precedes final permission"
        );
        assert!(!dir.join("denied.txt").exists());
        source.set_granted(BuiltinKind::Write, true).unwrap();

        let cancel = CancellationToken::new();
        cancel.cancel();
        let args = heap
            .import_serde(&json!({"path":"pre-cancel-write/forbidden.txt","content":"forbidden"}))
            .unwrap();
        let (invoker, call) = request(&source, &values, 1, "write", args, cancel, noop.clone());
        assert!(matches!(
            invoker.invoke_settled(call).await,
            Err(ActualProviderError::Callback(ref message)) if message == "Aborted: Cancelled"
        ));
        assert!(
            !dir.join("pre-cancel-write").exists(),
            "pre-cancelled Write creates neither directory nor file"
        );

        let cancel = CancellationToken::new();
        *runtime.cancel_write.lock().unwrap() = Some(cancel.clone());
        runtime.gets.lock().unwrap().clear();
        let args = heap.object().unwrap();
        for (key, id) in [("path", 9103), ("content", 9104)] {
            heap.define_own(
                &args,
                key.into(),
                PropertyDescriptor::Accessor {
                    get: Some(CallableTarget::Host(CapabilityId(id))),
                    set: None,
                    enumerable: true,
                    configurable: true,
                },
                values.runtime(),
            )
            .unwrap();
        }
        let (invoker, call) = request(
            &source,
            &values,
            1,
            "write",
            args,
            cancel.clone(),
            noop.clone(),
        );
        assert!(matches!(
            invoker.invoke_settled(call).await,
            Err(ActualProviderError::Callback(ref message)) if message == "Aborted: Cancelled"
        ));
        assert!(cancel.is_cancelled());
        assert_eq!(
            *runtime.gets.lock().unwrap(),
            ["path", "content"],
            "fixed parameter Get order survives cancellation"
        );
        assert!(
            !dir.join("getter-cancel-write").exists(),
            "getter cancellation precedes every Write OS effect"
        );
        runtime.cancel_write.lock().unwrap().take();

        let args = heap
            .import_serde(&json!({"path":"native.txt","content":"native\ncontent"}))
            .unwrap();
        let (invoker, call) = request(
            &source,
            &values,
            1,
            "write",
            args.clone(),
            CancellationToken::new(),
            noop.clone(),
        );
        let original_member = call.tool_call.clone();
        let result = invoker.invoke_settled(call).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("native.txt")).unwrap(),
            "native\ncontent"
        );
        assert!(
            heap.get(&original_member, &"arguments".into(), values.runtime())
                .unwrap()
                .same_identity(&args)
        );
        assert!(
            !result.same_identity(&args),
            "OS result is newly derived, original arguments stay canonical"
        );
        let read_args = heap.import_serde(&json!({"path":"native.txt"})).unwrap();
        let (invoker, call) = request(
            &source,
            &values,
            0,
            "read",
            read_args,
            CancellationToken::new(),
            noop.clone(),
        );
        let result = invoker.invoke_settled(call).await.unwrap();
        assert!(
            heap.export_serde(&result).unwrap()["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("native")
        );

        let args = heap
            .import_serde(&json!({"path":"wrong.txt","content":"wrong"}))
            .unwrap();
        let (invoker, mut call) = request(
            &source,
            &values,
            1,
            "write",
            args,
            CancellationToken::new(),
            noop.clone(),
        );
        let ValueSnapshot::Function(CallableTarget::Host(id)) =
            heap.snapshot(&call.callable).unwrap()
        else {
            panic!("Host method")
        };
        call.callable = heap.function(id).unwrap();
        assert!(matches!(
            invoker.invoke_settled(call).await,
            Err(ActualProviderError::Protocol(_))
        ));
        assert!(
            !dir.join("wrong.txt").exists(),
            "a different function with the same capability ID is not the selected callable"
        );
        let new_values = SessionValues::new(heap.clone(), runtime.clone()).unwrap();
        assert!(
            source.current_tools(&new_values).is_err(),
            "same heap alone does not replace the logical Session Arc"
        );

        // A thrown update must not abandon the already entered process Future.
        let witness = heap
            .string("original native update exception".into())
            .unwrap();
        let seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let update: ara_agent::ActualUpdateFn = Arc::new({
            let witness = witness.clone();
            let seen = seen.clone();
            move |_| {
                seen.fetch_add(1, Ordering::SeqCst);
                Err(ValueError::Thrown(witness.clone()).into())
            }
        });
        let args = heap
            .import_serde(&json!({"command":"printf started; sleep 0.65; printf settled > settled.txt","timeout":2}))
            .unwrap();
        let (invoker, call) = request(
            &source,
            &values,
            2,
            "bash",
            args,
            CancellationToken::new(),
            update,
        );
        let error = invoker.invoke_settled(call).await.unwrap_err();
        assert!(contains_throw(&error, &witness));
        assert!(
            seen.load(Ordering::SeqCst) > 0,
            "actual partial callback was invoked"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("settled.txt")).unwrap(),
            "settled",
            "wait for entered effect despite update failure"
        );

        let update: ara_agent::ActualUpdateFn = Arc::new({
            let source = source.clone();
            let witness = witness.clone();
            move |_| {
                source.set_granted(BuiltinKind::Bash, false)?;
                Err(ValueError::Thrown(witness.clone()).into())
            }
        });
        let args = heap
            .import_serde(&json!({"command":"printf revoke; sleep 1; printf forbidden > revoked.txt","timeout":2}))
            .unwrap();
        let (invoker, call) = request(
            &source,
            &values,
            2,
            "bash",
            args,
            CancellationToken::new(),
            update,
        );
        let error = invoker.invoke_settled(call).await.unwrap_err();
        assert!(
            contains_throw(&error, &witness),
            "retain update exception alongside process cancellation settlement"
        );
        assert!(matches!(error, ActualProviderError::Compound { .. }));
        assert!(!dir.join("revoked.txt").exists());
        source.set_granted(BuiltinKind::Bash, true).unwrap();

        let cancel = CancellationToken::new();
        cancel.cancel();
        let args = heap
            .import_serde(&json!({"command":"printf forbidden > pre-cancel.txt","timeout":2}))
            .unwrap();
        let (invoker, call) = request(&source, &values, 2, "bash", args, cancel, noop);
        assert!(invoker.invoke_settled(call).await.is_err());
        assert!(!dir.join("pre-cancel.txt").exists());
        // UNEXECUTED splice draft, not an additional test entry point.
        // Insert this block inside actual_builtin_invocation_observations(), before its
        // final remove_dir_all(&dir). It reuses that helper's request/contains_throw.
        // Root owns assembly, compile, execution and independent acceptance.
        {
            use std::collections::{HashMap, VecDeque};

            enum FourReply {
                Value(ValueHandle),
                Throw(ValueHandle),
                Revoke(BuiltinKind, ValueHandle),
                Cancel(CancellationToken, ValueHandle),
            }
            struct FourRuntime {
                source: std::sync::Mutex<Option<std::sync::Weak<BuiltinToolSource>>>,
                events: std::sync::Mutex<Vec<&'static str>>,
                replies:
                    std::sync::Mutex<HashMap<CapabilityId, (&'static str, VecDeque<FourReply>)>>,
            }
            impl ValueRuntime for FourRuntime {
                fn resolve(
                    &self,
                    request: RuntimeRequest<'_>,
                ) -> std::result::Result<RuntimeReply, ValueError> {
                    let source = self
                        .source
                        .lock()
                        .unwrap()
                        .as_ref()
                        .and_then(std::sync::Weak::upgrade);
                    if let Some(reply) = source
                        .as_ref()
                        .and_then(|source| source.resolve_context_signal(&request))
                    {
                        return reply;
                    }
                    let RuntimeRequest::Call { capability, .. } = request else {
                        return Err(ValueError::MissingCapability(request.capability()));
                    };
                    let (label, reply) = {
                        let mut replies = self.replies.lock().unwrap();
                        let (label, queue) = replies
                            .get_mut(&capability)
                            .ok_or(ValueError::MissingCapability(capability))?;
                        (
                            *label,
                            queue
                                .pop_front()
                                .expect("extra original getter/method Call"),
                        )
                    };
                    self.events.lock().unwrap().push(label);
                    match reply {
                        FourReply::Value(value) => Ok(RuntimeReply::Value(value)),
                        FourReply::Throw(witness) => Err(ValueError::Thrown(witness)),
                        FourReply::Revoke(kind, value) => {
                            source.unwrap().set_granted(kind, false).unwrap();
                            Ok(RuntimeReply::Value(value))
                        }
                        FourReply::Cancel(token, value) => {
                            token.cancel();
                            Ok(RuntimeReply::Value(value))
                        }
                    }
                }
            }
            fn four_getter(
                args: &ValueHandle,
                key: &str,
                id: u64,
                label: &'static str,
                replies: Vec<FourReply>,
                values: &Arc<SessionValues>,
                runtime: &FourRuntime,
            ) {
                runtime
                    .replies
                    .lock()
                    .unwrap()
                    .insert(CapabilityId(id), (label, replies.into()));
                values
                    .heap()
                    .define_own(
                        args,
                        key.into(),
                        PropertyDescriptor::Accessor {
                            get: Some(CallableTarget::Host(CapabilityId(id))),
                            set: None,
                            enumerable: true,
                            configurable: true,
                        },
                        values.runtime(),
                    )
                    .unwrap();
            }
            fn four_method(
                receiver: &ValueHandle,
                key: &str,
                id: u64,
                label: &'static str,
                reply: FourReply,
                values: &Arc<SessionValues>,
                runtime: &FourRuntime,
            ) {
                runtime
                    .replies
                    .lock()
                    .unwrap()
                    .insert(CapabilityId(id), (label, vec![reply].into()));
                values
                    .heap()
                    .define_own_data(
                        receiver,
                        key.into(),
                        values.heap().function(CapabilityId(id)).unwrap(),
                        values.runtime(),
                    )
                    .unwrap();
            }
            let four_dir = dir.join("four-tools");
            std::fs::create_dir(&four_dir).unwrap();
            std::fs::write(four_dir.join("sample.py"), "print('hello')\n").unwrap();
            let heap = ValueHeap::new().unwrap();
            let runtime = Arc::new(FourRuntime {
                source: Default::default(),
                events: Default::default(),
                replies: Default::default(),
            });
            let values = Arc::new(SessionValues::new(heap.clone(), runtime.clone()).unwrap());
            let source = BuiltinToolSource::new(
                &values,
                ToolContext::new(four_dir.clone()).with_edit(ara_edit::EditMode::Replace, true),
                &[
                    BuiltinKind::Edit,
                    BuiltinKind::Grep,
                    BuiltinKind::Glob,
                    BuiltinKind::AstGrep,
                ],
            )
            .unwrap();
            *runtime.source.lock().unwrap() = Some(Arc::downgrade(&source));
            let noop: ara_agent::ActualUpdateFn = Arc::new(|_| Ok(()));

            // Canonical Graph metadata: provider declarations read this source, not the
            // Rust BoundTool methods. Logical name/capability remain edit in every mode.
            let declaration_model = ara_ai::Model {
                id: "four-tools-metadata-fixture".into(),
                api: "openai-responses".into(),
                provider: "fixture".into(),
                base_url: "http://127.0.0.1:1".into(),
                reasoning: false,
                max_tokens: None,
                context_window: None,
                tokenizer: None,
            };
            for mode in [
                ara_edit::EditMode::Replace,
                ara_edit::EditMode::Patch,
                ara_edit::EditMode::ApplyPatch,
                ara_edit::EditMode::Hashline,
                ara_edit::EditMode::Sloppy,
            ] {
                let edit_source = BuiltinToolSource::new(
                    &values,
                    ToolContext::new(four_dir.clone()).with_edit(mode, true),
                    &[BuiltinKind::Edit],
                )
                .unwrap();
                // Each dynamic metadata getter belongs to this newly constructed
                // source, not the four-tool invocation source retained below.
                *runtime.source.lock().unwrap() = Some(Arc::downgrade(&edit_source));
                let collection = edit_source.current_tools(&values).unwrap().unwrap();
                let selected = heap
                    .get(&collection, &"0".into(), values.runtime())
                    .unwrap();
                let logical = heap
                    .get(&selected, &"name".into(), values.runtime())
                    .unwrap();
                assert!(
                    matches!(heap.snapshot(&logical).unwrap(), ValueSnapshot::String(name) if name.equals_ascii("edit"))
                );
                let binding = edit_source.bind_selected(&selected, &values).unwrap();
                let format = heap
                    .get(&selected, &"customFormat".into(), values.runtime())
                    .unwrap();
                let wire_name = heap
                    .get(&selected, &"customWireName".into(), values.runtime())
                    .unwrap();
                let grammar = ara_edit::grammar(mode);
                if let Some(grammar) = grammar {
                    assert_eq!(
                        heap.export_serde(&format).unwrap(),
                        json!({"syntax":"lark","definition":grammar})
                    );
                } else {
                    assert!(matches!(
                        heap.snapshot(&format).unwrap(),
                        ValueSnapshot::Undefined
                    ));
                }
                if mode == ara_edit::EditMode::ApplyPatch {
                    assert!(
                        matches!(heap.snapshot(&wire_name).unwrap(), ValueSnapshot::String(name) if name.equals_ascii("apply_patch"))
                    );
                } else {
                    assert!(matches!(
                        heap.snapshot(&wire_name).unwrap(),
                        ValueSnapshot::Undefined
                    ));
                }
                let definition = binding.capability.protocol_definition();
                assert_eq!(
                    definition
                        .custom_format
                        .as_ref()
                        .map(|format| format.definition.as_str()),
                    grammar
                );
                assert_eq!(
                    definition.custom_wire_name.as_deref(),
                    (mode == ara_edit::EditMode::ApplyPatch).then_some("apply_patch")
                );
                let original_context = heap
                    .import_serde(&json!({"systemPrompt":[], "messages":[]}))
                    .unwrap();
                heap.define_own_data(
                    &original_context,
                    "tools".into(),
                    collection,
                    values.runtime(),
                )
                .unwrap();
                let context = ara_ai::GraphProviderContext {
                    original_context: Some(original_context),
                    system_prompt: Vec::new(),
                    messages: Vec::new(),
                    tools: None,
                    history_demotion_dialect: None,
                    history_id_hash: None,
                    history_id_hash_function: None,
                };
                for freeform in [false, true] {
                    let options = ara_ai::providers::openai_responses::RequestOptions {
                        apply_patch_tool_type: Some(
                            if freeform { "freeform" } else { "function" }.into(),
                        ),
                        ..Default::default()
                    };
                    let declaration = ara_ai::providers::openai_responses::actual::build_request(
                        &declaration_model,
                        &context,
                        values.factory().clone(),
                        &options,
                        &Default::default(),
                        ara_ai::providers::openai_responses::actual::GraphResponsesFlavor::Compatible,
                    )
                    .unwrap();
                    // Observe the declaration fields this fixture asserts through
                    // the original Get path. The request root retains private
                    // chain metadata Symbols outside its HTTP JSON payload.
                    let tools = heap
                        .get(&declaration, &"tools".into(), values.runtime())
                        .unwrap();
                    let native_tool = heap.get(&tools, &"0".into(), values.runtime()).unwrap();
                    let custom = freeform && grammar.is_some();
                    let tool = json!({
                        "type": heap.export_serde(&heap.get(&native_tool, &"type".into(), values.runtime()).unwrap()).unwrap(),
                        "name": heap.export_serde(&heap.get(&native_tool, &"name".into(), values.runtime()).unwrap()).unwrap(),
                        "format": if custom {
                            heap.export_serde(&heap.get(&native_tool, &"format".into(), values.runtime()).unwrap()).unwrap()
                        } else {
                            serde_json::Value::Null
                        },
                    });
                    assert_eq!(
                        tool["type"],
                        json!(if custom { "custom" } else { "function" })
                    );
                    assert_eq!(
                        tool["name"],
                        json!(if custom && mode == ara_edit::EditMode::ApplyPatch {
                            "apply_patch"
                        } else {
                            "edit"
                        })
                    );
                    if custom {
                        assert_eq!(tool["format"]["type"], json!("grammar"));
                        assert_eq!(tool["format"]["syntax"], json!("lark"));
                        assert!(!tool["format"]["definition"].as_str().unwrap().is_empty());
                    }
                }
            }

            *runtime.source.lock().unwrap() = Some(Arc::downgrade(&source));

            // Drive the public Raw Agent with a controlled actual provider, so the real
            // Core name/alias selection reaches the same advertised Edit capability.
            // This is deterministic source/dispatch evidence, not a real model trial.
            {
                struct FourAliasProvider {
                    call: ValueHandle,
                }
                impl ara_ai::ModelProvider for FourAliasProvider {
                    fn stream(
                        &self,
                        _: &ara_ai::Model,
                        _: &ara_ai::Context,
                        _: ara_ai::CallOptions,
                    ) -> ara_ai::AssistantStream {
                        panic!("alias fixture must use the actual provider entry");
                    }
                    fn stream_actual(
                        &self,
                        model: &ara_ai::Model,
                        _: &ara_ai::GraphProviderContext,
                        factory: Arc<ara_ai::graph_values::GraphValueFactory>,
                        _: ara_ai::CallOptions,
                    ) -> Result<ara_ai::ActualAssistantStream, ValueError> {
                        let heap = factory.heap();
                        heap.adopt(&self.call)?;
                        let mut assistant = serde_json::to_value(ara_ai::AssistantMessage::empty(
                            &model.api,
                            &model.provider,
                            &model.id,
                        ))
                        .unwrap();
                        assistant["role"] = json!("assistant");
                        assistant["stopReason"] = json!("toolUse");
                        let root = heap.import_serde(&assistant)?;
                        heap.define_own_data(
                            &root,
                            "content".into(),
                            heap.array(vec![self.call.clone()])?,
                            factory.runtime(),
                        )?;
                        let (tx, rx) = tokio::sync::mpsc::channel(4);
                        tx.try_send(ara_ai::ActualAssistantEvent::Start {
                            partial: root.clone(),
                        })
                        .unwrap();
                        tx.try_send(ara_ai::ActualAssistantEvent::ToolcallEnd {
                            content_index: heap.number(0.0)?,
                            tool_call: self.call.clone(),
                            partial: root.clone(),
                        })
                        .unwrap();
                        tx.try_send(ara_ai::ActualAssistantEvent::Done {
                            reason: ara_ai::StopReason::ToolUse,
                            message: root,
                        })
                        .unwrap();
                        drop(tx);
                        Ok(rx.into())
                    }
                }
                impl ara_agent::OwnedProviderContextPort for FourAliasProvider {
                    fn project_context_actual(
                        &self,
                        _: &ara_ai::Model,
                        context: ara_agent::ActualProviderContext,
                        _: bool,
                        _: &SessionValues,
                    ) -> Result<ara_agent::ActualProviderContext, ActualProviderError>
                    {
                        Ok(context)
                    }
                    fn stream_prepared_original_actual(
                        &self,
                        _: &dyn ara_ai::ModelProvider,
                        model: &ara_ai::Model,
                        context: &ara_agent::ActualProviderContext,
                        _: Option<ValueHandle>,
                        values: Arc<SessionValues>,
                        options: ara_ai::CallOptions,
                    ) -> Result<ara_ai::ActualAssistantStream, ActualProviderError>
                    {
                        let graph_context = ara_ai::GraphProviderContext {
                            original_context: context.original_context.clone(),
                            system_prompt: context.system_prompt.clone(),
                            messages: context.messages.clone(),
                            tools: context.tools.clone(),
                            history_demotion_dialect: None,
                            history_id_hash: None,
                            history_id_hash_function: None,
                        };
                        ara_ai::ModelProvider::stream_actual(
                            self,
                            model,
                            &graph_context,
                            values.factory().clone(),
                            options,
                        )
                        .map_err(Into::into)
                    }
                    fn project_context(
                        &self,
                        _: &ara_ai::Model,
                        _: ara_ai::Context,
                        _: bool,
                    ) -> Result<ara_ai::Context, ara_agent::ProviderContextError>
                    {
                        panic!("typed alias projection")
                    }
                    fn stream(
                        &self,
                        _: &dyn ara_ai::ModelProvider,
                        _: &ara_ai::Model,
                        _: &ara_ai::Context,
                        _: ara_ai::CallOptions,
                    ) -> tokio::sync::mpsc::Receiver<ara_ai::AssistantMessageEvent>
                    {
                        panic!("typed alias stream")
                    }
                }
                struct FourAliasHooks {
                    values: Arc<SessionValues>,
                    source: Arc<BuiltinToolSource>,
                    provider: Arc<FourAliasProvider>,
                    hooks: Arc<NativeBuiltinToolHooks>,
                    selected: ValueHandle,
                    capability: Arc<dyn ara_agent::AgentTool>,
                    arguments: ValueHandle,
                    admissions: std::sync::atomic::AtomicUsize,
                }
                #[async_trait::async_trait]
                impl ara_agent::LoopHooks for FourAliasHooks {
                    fn provider_context_binding(
                        &self,
                    ) -> Option<ara_agent::ProviderContextBinding> {
                        Some(ara_agent::ProviderContextBinding {
                            actual_values: Some(self.values.clone()),
                            owned_port: Some(self.provider.clone()),
                            ..Default::default()
                        })
                    }
                    fn actual_tool_source(
                        &self,
                        values: &SessionValues,
                    ) -> Result<Option<Arc<dyn ActualToolSource>>, ActualProviderError>
                    {
                        self.hooks.source(values).map(Some)
                    }
                    async fn convert_agent_messages_to_llm_actual(
                        &self,
                        messages: &[AgentMessage],
                        values: &SessionValues,
                    ) -> Result<Vec<ValueHandle>, ActualProviderError> {
                        messages
                            .iter()
                            .map(|message| match message.body_snapshot() {
                                ara_agent::AgentMessageBody::Graph(root) => {
                                    values.check_handle(&root)?;
                                    Ok(root)
                                }
                                _ => panic!("alias fixture cannot bridge a typed transcript"),
                            })
                            .collect()
                    }
                    async fn transform_provider_context_actual(
                        &self,
                        context: ara_agent::ActualProviderContext,
                        _: &ara_ai::Model,
                        _: &SessionValues,
                    ) -> Result<ara_agent::ActualProviderContext, ActualProviderError>
                    {
                        Ok(context)
                    }
                    async fn transform_assistant_graph(
                        &self,
                        _: &AgentMessage,
                        _: &CancellationToken,
                        _: &SessionValues,
                    ) -> Result<(), ActualProviderError> {
                        Ok(())
                    }
                    fn supports_actual_turn_end(&self) -> bool {
                        true
                    }
                    async fn turn_end_actual(
                        &self,
                        _: &AgentMessage,
                        _: &[AgentMessage],
                        _: &CancellationToken,
                        _: &SessionValues,
                    ) -> Result<Vec<AgentMessage>, ActualProviderError> {
                        Ok(Vec::new())
                    }
                    fn supports_actual_tool_hooks(&self) -> bool {
                        true
                    }
                    async fn before_tool_dispatch_actual(
                        &self,
                        _: ara_agent::ActualBeforeToolCallContext<'_>,
                        _: &CancellationToken,
                    ) -> Result<Option<ValueHandle>, ActualProviderError> {
                        Ok(None)
                    }
                    async fn before_tool_call_actual(
                        &self,
                        context: ara_agent::ActualBeforeToolCallContext<'_>,
                        _: &CancellationToken,
                    ) -> Result<ara_agent::ActualToolDecision, ActualProviderError>
                    {
                        assert!(context.tool.source.same_identity(&self.selected));
                        assert!(Arc::ptr_eq(&context.tool.capability, &self.capability));
                        assert!(context.args.same_identity(&self.arguments));
                        self.admissions
                            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        self.source.permission(context.tool, context.values)
                    }
                    async fn after_tool_call_actual(
                        &self,
                        _: ara_agent::ActualAfterToolCallContext<'_>,
                        _: &CancellationToken,
                    ) -> Result<Option<ValueHandle>, ActualProviderError> {
                        Ok(None)
                    }
                    fn transform_tool_call_arguments_actual(
                        &self,
                        args: ValueHandle,
                        _: &ara_rpc::WireString,
                        values: &SessionValues,
                    ) -> Result<ValueHandle, ActualProviderError> {
                        values.check_handle(&args)?;
                        Ok(args)
                    }
                    fn get_tool_context_actual(
                        &self,
                        context: &ara_agent::ActualToolExecutionContext,
                        values: &SessionValues,
                    ) -> Result<ValueHandle, ActualProviderError> {
                        self.hooks.tool_context(context, values)
                    }
                    fn sanitize_tool_result_text_actual(
                        &self,
                        text: &ValueHandle,
                        values: &SessionValues,
                    ) -> Result<ValueHandle, ActualProviderError> {
                        values.check_handle(text)?;
                        Ok(text.clone())
                    }
                }
                let patch_source = BuiltinToolSource::new(
                    &values,
                    ToolContext::new(four_dir.clone())
                        .with_edit(ara_edit::EditMode::ApplyPatch, true),
                    &[BuiltinKind::Edit],
                )
                .unwrap();
                *runtime.source.lock().unwrap() = Some(Arc::downgrade(&patch_source));
                let collection = patch_source.current_tools(&values).unwrap().unwrap();
                let selected = heap
                    .get(&collection, &"0".into(), values.runtime())
                    .unwrap();
                let binding = patch_source.bind_selected(&selected, &values).unwrap();
                let args = heap
                    .import_serde(&json!({"input":"*** Begin Patch\n*** Add File: alias.txt\n+alias\n*** End Patch"}))
                    .unwrap();
                let call = heap.import_serde(&json!({"type":"toolCall","id":"four-alias","name":"apply_patch","customWireName":"apply_patch"})).unwrap();
                heap.define_own_data(&call, "arguments".into(), args.clone(), values.runtime())
                    .unwrap();
                let alias_provider = Arc::new(FourAliasProvider { call });
                let alias_hooks = Arc::new(FourAliasHooks {
                    values: values.clone(),
                    source: patch_source.clone(),
                    provider: alias_provider.clone(),
                    hooks: NativeBuiltinToolHooks::new(
                        patch_source.clone(),
                        None,
                        None,
                        None,
                        &values,
                    )
                    .unwrap(),
                    selected,
                    capability: binding.capability,
                    arguments: args,
                    admissions: Default::default(),
                });
                let config = ara_agent::AgentConfig {
                    model: declaration_model.clone(),
                    provider: alias_provider,
                    system_prompt: Vec::new(),
                    tools: patch_source.tools(),
                    tool_choice: None,
                    max_tokens: None,
                    temperature: None,
                    deadline: None,
                    max_model_calls: Some(1),
                    hooks: alias_hooks.clone(),
                    interrupt_mode: Default::default(),
                };
                let input = AgentMessage::from_graph(
                    heap.import_serde(
                        &json!({"role":"user","content":"alias fixture","timestamp":1}),
                    )
                    .unwrap(),
                    None,
                )
                .unwrap();
                let agent = ara_agent::Agent::new_raw(config, Vec::new()).unwrap();
                let report = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    agent.prompt_raw(
                        vec![input],
                        CancellationToken::new(),
                        Arc::new(ara_agent::NullSink),
                    ),
                )
                .await
                .unwrap()
                .unwrap();
                assert_eq!(report.end, ara_agent::RunEnd::ModelCallBudget);
                assert_eq!(
                    alias_hooks
                        .admissions
                        .load(std::sync::atomic::Ordering::SeqCst),
                    1
                );
                assert_eq!(
                    std::fs::read_to_string(four_dir.join("alias.txt")).unwrap(),
                    "alias\n"
                );
                *runtime.source.lock().unwrap() = Some(Arc::downgrade(&source));
            }

            // Edit consumes original toJSON; ignored original fields are not exported.
            let replacement = heap
                .import_serde(&json!({
                    "path":"sample.py", "old_string":"hello", "new_string":"HELLO"
                }))
                .unwrap();
            let args = heap.object().unwrap();
            four_method(
                &args,
                "toJSON",
                9201,
                "edit.toJSON",
                FourReply::Value(replacement),
                &values,
                &runtime,
            );
            heap.define_own_data(
                &args,
                "ignoredBigInt".into(),
                heap.bigint_decimal("1").unwrap(),
                values.runtime(),
            )
            .unwrap();
            let (invoker, call) = request(
                &source,
                &values,
                0,
                "edit",
                args.clone(),
                CancellationToken::new(),
                noop.clone(),
            );
            let member = call.tool_call.clone();
            let result = invoker.invoke_settled(call).await.unwrap();
            values.check_handle(&result).unwrap();
            assert!(!result.same_identity(&args));
            assert!(
                heap.get(&member, &"arguments".into(), values.runtime())
                    .unwrap()
                    .same_identity(&args)
            );
            assert_eq!(
                std::fs::read_to_string(four_dir.join("sample.py")).unwrap(),
                "print('HELLO')\n"
            );
            assert_eq!(*runtime.events.lock().unwrap(), ["edit.toJSON"]);

            // Grep gets its five destructured fields once, in fixed source order.
            runtime.events.lock().unwrap().clear();
            let args = heap.object().unwrap();
            for (key, id, value) in [
                ("pattern", 9210, heap.string("HELLO".into()).unwrap()),
                ("path", 9211, heap.string("sample.py".into()).unwrap()),
                ("case", 9212, heap.boolean(true).unwrap()),
                ("gitignore", 9213, heap.boolean(true).unwrap()),
                ("skip", 9214, heap.number(0.9).unwrap()),
            ] {
                four_getter(
                    &args,
                    key,
                    id,
                    key,
                    vec![FourReply::Value(value)],
                    &values,
                    &runtime,
                );
            }
            let (invoker, call) = request(
                &source,
                &values,
                1,
                "grep",
                args,
                CancellationToken::new(),
                noop.clone(),
            );
            let result = invoker.invoke_settled(call).await.unwrap();
            values.check_handle(&result).unwrap();
            assert!(
                heap.export_serde(&result).unwrap()["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .contains("HELLO")
            );
            assert_eq!(
                *runtime.events.lock().unwrap(),
                ["pattern", "path", "case", "gitignore", "skip"]
            );

            // Glob binds four fields before launch; fresh output is in this Session.
            runtime.events.lock().unwrap().clear();
            let args = heap.object().unwrap();
            for (key, id, value) in [
                ("path", 9220, heap.string("*.py".into()).unwrap()),
                ("limit", 9221, heap.number(1.9).unwrap()),
                ("hidden", 9222, heap.boolean(true).unwrap()),
                ("gitignore", 9223, heap.boolean(true).unwrap()),
            ] {
                four_getter(
                    &args,
                    key,
                    id,
                    key,
                    vec![FourReply::Value(value)],
                    &values,
                    &runtime,
                );
            }
            let (invoker, call) = request(
                &source,
                &values,
                2,
                "glob",
                args,
                CancellationToken::new(),
                noop.clone(),
            );
            let result = invoker.invoke_settled(call).await.unwrap();
            values.check_handle(&result).unwrap();
            assert!(
                heap.export_serde(&result).unwrap()["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .contains("sample.py")
            );
            assert_eq!(
                *runtime.events.lock().unwrap(),
                ["path", "limit", "hidden", "gitignore"]
            );

            // AST reads a defined skip twice. Only the second value undergoes ToNumber.
            runtime.events.lock().unwrap().clear();
            let args = heap.object().unwrap();
            let skip_object = heap.object().unwrap();
            four_method(
                &skip_object,
                "valueOf",
                9234,
                "skip.valueOf",
                FourReply::Value(heap.number(0.9).unwrap()),
                &values,
                &runtime,
            );
            four_getter(
                &args,
                "pat",
                9230,
                "pat",
                vec![FourReply::Value(
                    heap.string(" print($$$A) ".into()).unwrap(),
                )],
                &values,
                &runtime,
            );
            four_getter(
                &args,
                "skip",
                9231,
                "skip",
                vec![
                    FourReply::Value(heap.null().unwrap()),
                    FourReply::Value(skip_object),
                ],
                &values,
                &runtime,
            );
            four_getter(
                &args,
                "path",
                9232,
                "path",
                vec![FourReply::Value(heap.string("sample.py".into()).unwrap())],
                &values,
                &runtime,
            );
            let (invoker, call) = request(
                &source,
                &values,
                3,
                "ast_grep",
                args,
                CancellationToken::new(),
                noop.clone(),
            );
            let result = invoker.invoke_settled(call).await.unwrap();
            values.check_handle(&result).unwrap();
            assert!(
                heap.export_serde(&result).unwrap()["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .contains("print")
            );
            assert_eq!(
                *runtime.events.lock().unwrap(),
                ["pat", "skip", "skip", "skip.valueOf", "path"]
            );

            // Original trim throw survives; no OS result or copied exception replaces it.
            runtime.events.lock().unwrap().clear();
            let witness = heap.object().unwrap();
            let pattern = heap.object().unwrap();
            four_method(
                &pattern,
                "trim",
                9240,
                "pattern.trim",
                FourReply::Throw(witness.clone()),
                &values,
                &runtime,
            );
            let args = heap.object().unwrap();
            heap.define_own_data(&args, "pattern".into(), pattern, values.runtime())
                .unwrap();
            let (invoker, call) = request(
                &source,
                &values,
                1,
                "grep",
                args,
                CancellationToken::new(),
                noop.clone(),
            );
            assert!(contains_throw(
                &invoker.invoke_settled(call).await.unwrap_err(),
                &witness
            ));
            assert_eq!(*runtime.events.lock().unwrap(), ["pattern.trim"]);

            // A getter exception stops later destructuring Gets and preserves identity.
            runtime.events.lock().unwrap().clear();
            let args = heap.object().unwrap();
            four_getter(
                &args,
                "pattern",
                9241,
                "pattern",
                vec![FourReply::Value(heap.string("HELLO".into()).unwrap())],
                &values,
                &runtime,
            );
            four_getter(
                &args,
                "path",
                9242,
                "path",
                vec![FourReply::Throw(witness.clone())],
                &values,
                &runtime,
            );
            four_getter(
                &args,
                "case",
                9243,
                "case",
                vec![FourReply::Value(heap.boolean(true).unwrap())],
                &values,
                &runtime,
            );
            let (invoker, call) = request(
                &source,
                &values,
                1,
                "grep",
                args,
                CancellationToken::new(),
                noop.clone(),
            );
            assert!(contains_throw(
                &invoker.invoke_settled(call).await.unwrap_err(),
                &witness
            ));
            assert_eq!(*runtime.events.lock().unwrap(), ["pattern", "path"]);

            // Already-cancelled Grep still destructures; AST does not enter its closure.
            runtime.events.lock().unwrap().clear();
            let cancel = CancellationToken::new();
            cancel.cancel();
            let args = heap.object().unwrap();
            for (key, id, value) in [
                ("pattern", 9260, heap.string("HELLO".into()).unwrap()),
                ("path", 9261, heap.string("sample.py".into()).unwrap()),
                ("case", 9262, heap.boolean(true).unwrap()),
                ("gitignore", 9263, heap.boolean(true).unwrap()),
                ("skip", 9264, heap.number(0.0).unwrap()),
            ] {
                four_getter(
                    &args,
                    key,
                    id,
                    key,
                    vec![FourReply::Value(value)],
                    &values,
                    &runtime,
                );
            }
            let (invoker, call) = request(
                &source,
                &values,
                1,
                "grep",
                args,
                cancel.clone(),
                noop.clone(),
            );
            assert!(
                matches!(invoker.invoke_settled(call).await, Err(ActualProviderError::Callback(ref message)) if message == "Aborted: Cancelled")
            );
            assert_eq!(
                *runtime.events.lock().unwrap(),
                ["pattern", "path", "case", "gitignore", "skip"]
            );
            runtime.events.lock().unwrap().clear();
            let args = heap.object().unwrap();
            four_getter(
                &args,
                "pat",
                9265,
                "pat",
                vec![FourReply::Throw(witness.clone())],
                &values,
                &runtime,
            );
            let (invoker, call) = request(
                &source,
                &values,
                3,
                "ast_grep",
                args,
                cancel.clone(),
                noop.clone(),
            );
            assert!(
                matches!(invoker.invoke_settled(call).await, Err(ActualProviderError::Callback(ref message)) if message == "Aborted: Cancelled")
            );
            assert!(runtime.events.lock().unwrap().is_empty());
            let args = heap.object().unwrap();
            for (key, id, value) in [
                ("path", 9270, heap.string("*.py".into()).unwrap()),
                ("limit", 9271, heap.number(1.0).unwrap()),
                ("hidden", 9272, heap.boolean(true).unwrap()),
                ("gitignore", 9273, heap.boolean(true).unwrap()),
            ] {
                four_getter(
                    &args,
                    key,
                    id,
                    key,
                    vec![FourReply::Value(value)],
                    &values,
                    &runtime,
                );
            }
            let (invoker, call) = request(&source, &values, 2, "glob", args, cancel, noop.clone());
            assert!(
                matches!(invoker.invoke_settled(call).await, Err(ActualProviderError::Callback(ref message)) if message == "Aborted: Cancelled")
            );
            assert_eq!(
                *runtime.events.lock().unwrap(),
                ["path", "limit", "hidden", "gitignore"]
            );

            // Edit.stringify runs before the final grant gate, then performs no write.
            runtime.events.lock().unwrap().clear();
            let args = heap
                .import_serde(&json!({"path":"sample.py", "old_string":"HELLO"}))
                .unwrap();
            four_getter(
                &args,
                "new_string",
                9250,
                "new_string",
                vec![FourReply::Revoke(
                    BuiltinKind::Edit,
                    heap.string("FORBIDDEN".into()).unwrap(),
                )],
                &values,
                &runtime,
            );
            let (invoker, call) = request(
                &source,
                &values,
                0,
                "edit",
                args,
                CancellationToken::new(),
                noop.clone(),
            );
            assert!(
                matches!(invoker.invoke_settled(call).await, Err(ActualProviderError::Callback(ref message)) if message.contains("current Host grant"))
            );
            assert_eq!(
                std::fs::read_to_string(four_dir.join("sample.py")).unwrap(),
                "print('HELLO')\n"
            );
            source.set_granted(BuiltinKind::Edit, true).unwrap();

            // Cancellation from original stringify Getter is observed by the real writer.
            let cancel = CancellationToken::new();
            let args = heap
                .import_serde(&json!({"path":"sample.py", "old_string":"HELLO"}))
                .unwrap();
            four_getter(
                &args,
                "new_string",
                9251,
                "new_string",
                vec![FourReply::Cancel(
                    cancel.clone(),
                    heap.string("FORBIDDEN".into()).unwrap(),
                )],
                &values,
                &runtime,
            );
            let (invoker, call) = request(
                &source,
                &values,
                0,
                "edit",
                args,
                cancel.clone(),
                noop.clone(),
            );
            let result = invoker.invoke_settled(call).await.unwrap();
            assert!(cancel.is_cancelled());
            assert_eq!(heap.export_serde(&result).unwrap()["isError"], json!(true));
            assert_eq!(
                std::fs::read_to_string(four_dir.join("sample.py")).unwrap(),
                "print('HELLO')\n"
            );

            // Plain Edit.stringify does not enable the Agent's BigInt replacer.
            let args = heap
                .import_serde(
                    &json!({"path":"sample.py", "old_string":"HELLO", "new_string":"FORBIDDEN"}),
                )
                .unwrap();
            heap.define_own_data(
                &args,
                "extra".into(),
                heap.bigint_decimal("1").unwrap(),
                values.runtime(),
            )
            .unwrap();
            let (invoker, call) = request(
                &source,
                &values,
                0,
                "edit",
                args,
                CancellationToken::new(),
                noop.clone(),
            );
            assert!(matches!(
                invoker.invoke_settled(call).await,
                Err(ActualProviderError::Value(ValueError::TypeError(_)))
            ));
            let args = heap
                .import_serde(
                    &json!({"path":"sample.py", "old_string":"HELLO", "new_string":"FORBIDDEN"}),
                )
                .unwrap();
            heap.define_own_data(&args, "cycle".into(), args.clone(), values.runtime())
                .unwrap();
            let (invoker, call) = request(
                &source,
                &values,
                0,
                "edit",
                args,
                CancellationToken::new(),
                noop.clone(),
            );
            assert!(invoker.invoke_settled(call).await.is_err());
            assert_eq!(
                std::fs::read_to_string(four_dir.join("sample.py")).unwrap(),
                "print('HELLO')\n"
            );

            // Pin actual search workers in the existing skill resolver. This performs
            // no artificial production hook or kernel change. The explicit first poll
            // reaches spawn_blocking: invoke_settled/start/action/execute_os_values have
            // no earlier suspension on these inputs; the held write lock prevents the
            // spawned real worker from completing its required skill-resolution read.
            // Constructing an unpolled Future would NOT constitute launch evidence.
            for kind in [BuiltinKind::Grep, BuiltinKind::AstGrep] {
                for revoke in [false, true] {
                    let context = ToolContext::new(four_dir.clone()).with_skills(vec![
                        ara_tools::internal_urls::SkillRef {
                            name: "drain".into(),
                            file_path: four_dir.join("SKILL.md"),
                            base_dir: four_dir.clone(),
                        },
                    ]);
                    let store = context.edit_store.clone();
                    let skills = context.skills.clone();
                    let held = skills.write().unwrap();
                    let blocked_source = BuiltinToolSource::new(&values, context, &[kind]).unwrap();
                    *runtime.source.lock().unwrap() = Some(Arc::downgrade(&blocked_source));
                    let cancel = CancellationToken::new();
                    let (name, args) = match kind {
                        BuiltinKind::Grep => (
                            "grep",
                            json!({"pattern":"HELLO","path":"skill://drain/sample.py"}),
                        ),
                        BuiltinKind::AstGrep => (
                            "ast_grep",
                            json!({"pat":"print($$$A)","path":"skill://drain/sample.py"}),
                        ),
                        _ => unreachable!(),
                    };
                    let (invoker, call) = request(
                        &blocked_source,
                        &values,
                        0,
                        name,
                        heap.import_serde(&args).unwrap(),
                        cancel.clone(),
                        noop.clone(),
                    );
                    let mut entered = Box::pin(invoker.invoke_settled(call));
                    assert!(
                        futures::poll!(entered.as_mut()).is_pending(),
                        "first poll launches the pinned original worker"
                    );
                    if revoke {
                        blocked_source.set_granted(kind, false).unwrap();
                    } else {
                        cancel.cancel();
                    }
                    // Preserve the original owned Future after this bounded poll. Drop
                    // only timeout's temporary &mut borrow, then release the real lock.
                    let early = tokio::time::timeout(
                        std::time::Duration::from_millis(20),
                        entered.as_mut(),
                    )
                    .await;
                    drop(held);
                    assert!(
                        early.is_err(),
                        "cancel/revoke cannot settle while the launched worker is pinned: {name}/{revoke}"
                    );
                    let result =
                        tokio::time::timeout(std::time::Duration::from_secs(5), entered.as_mut())
                            .await
                            .unwrap();
                    assert!(
                        matches!(result, Err(ActualProviderError::Callback(ref message)) if message == if kind == BuiltinKind::Grep { "Grep was aborted" } else { "AST grep was aborted" })
                    );
                    let sample = ara_edit::path_policy::canonical_key(&four_dir.join("sample.py"));
                    assert!(store.head(&sample).is_none());
                    // The same shared store remains usable after the settled receipt.
                    // A new regular-file call records the snapshot and displayed lines;
                    // the previous worker is no longer allowed to mutate that state.
                    blocked_source.set_granted(kind, true).unwrap();
                    let args = match kind {
                        BuiltinKind::Grep => json!({"pattern":"HELLO","path":"sample.py"}),
                        BuiltinKind::AstGrep => json!({"pat":"print($$$A)","path":"sample.py"}),
                        _ => unreachable!(),
                    };
                    let (invoker, call) = request(
                        &blocked_source,
                        &values,
                        0,
                        name,
                        heap.import_serde(&args).unwrap(),
                        CancellationToken::new(),
                        noop.clone(),
                    );
                    invoker.invoke_settled(call).await.unwrap();
                    let settled = store.head(&sample).unwrap();
                    assert_eq!(&*settled.text, "print('HELLO')\n");
                    assert!(
                        settled
                            .seen_lines
                            .as_ref()
                            .is_some_and(|lines| lines.contains(&1))
                    );
                    tokio::task::yield_now().await;
                    let after = store.head(&sample).unwrap();
                    assert_eq!(after.hash, settled.hash);
                    assert_eq!(after.text, settled.text);
                    assert_eq!(after.seen_lines, settled.seen_lines);
                }
            }
            *runtime.source.lock().unwrap() = Some(Arc::downgrade(&source));

            // Explicitly admitted subsets must never silently serialize a path iterable.
            let args = heap
                .import_serde(&json!({"pattern":"HELLO", "path":["sample.py"]}))
                .unwrap();
            let (invoker, call) = request(
                &source,
                &values,
                1,
                "grep",
                args,
                CancellationToken::new(),
                noop,
            );
            assert!(matches!(
                invoker.invoke_settled(call).await,
                Err(ActualProviderError::Unsupported(
                    "search original path iterable action boundary"
                ))
            ));
        }

        std::fs::remove_dir_all(&dir).unwrap();
    }

    // Invoked from the existing journal-failure family; no new test entrance.
    async fn actual_public_boundary_and_legacy_preflight_observations() {
        use ara_agent::SessionValues;
        use ara_rpc::value_heap::{
            CallableTarget, CapabilityId, NoRuntime, PropertyDescriptor, RuntimeReply,
            RuntimeRequest, ValueError, ValueHandle, ValueHeap, ValueRuntime, ValueSnapshot,
        };
        use serde_json::json;
        struct ToJsonRuntime {
            roots: std::sync::Mutex<Vec<ValueHandle>>,
            calls: std::sync::atomic::AtomicUsize,
            keys: std::sync::Mutex<Vec<ara_rpc::WireString>>,
        }
        impl ValueRuntime for ToJsonRuntime {
            fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
                match request {
                    RuntimeRequest::Call {
                        capability: CapabilityId(id @ (8101 | 8102)),
                        ..
                    } => {
                        self.calls.fetch_add(1, Ordering::SeqCst);
                        Ok(RuntimeReply::Value(
                            self.roots.lock().unwrap()[(id - 8101) as usize].clone(),
                        ))
                    }
                    RuntimeRequest::Call {
                        capability: CapabilityId(id @ (8111 | 8112)),
                        arguments,
                        ..
                    } => {
                        let ValueSnapshot::String(key) =
                            arguments[0].heap().snapshot(&arguments[0])?
                        else {
                            panic!("native toJSON property key")
                        };
                        self.keys.lock().unwrap().push(key);
                        Ok(RuntimeReply::Value(
                            self.roots.lock().unwrap()[(id - 8109) as usize].clone(),
                        ))
                    }
                    RuntimeRequest::Call {
                        capability: CapabilityId(8113),
                        receiver,
                        ..
                    } => Ok(RuntimeReply::Value(
                        receiver.heap().function(CapabilityId(8111))?,
                    )),
                    request => Err(ValueError::MissingCapability(request.capability())),
                }
            }
        }
        let heap = ValueHeap::new().unwrap();
        let runtime = Arc::new(ToJsonRuntime {
            roots: std::sync::Mutex::new(Vec::new()),
            calls: Default::default(),
            keys: Default::default(),
        });
        let values = Arc::new(SessionValues::new(heap.clone(), runtime.clone()).unwrap());
        let mut message = serde_json::to_value(Message::Assistant(
            ara_ai::AssistantMessage::empty("a", "p", "m"),
        ))
        .unwrap();
        message["providerPayload"] = json!({"private":"provider"});
        message["content"] = json!([
            {"type":"thinking","thinking":"source thought","thinkingSignature":"private signature"},
            {"type":"toolCall","id":"call","name":"read","arguments":{"providerPayload":"business","thinkingSignature":"business signature"}},
        ]);
        let root = heap.import_serde(&message).unwrap();
        let content = heap.get(&root, &"content".into(), &NoRuntime).unwrap();
        let thinking = heap.get(&content, &"0".into(), &NoRuntime).unwrap();
        *runtime.roots.lock().unwrap() = vec![root.clone(), thinking.clone()];
        heap.define_own_data(
            &root,
            "toJSON".into(),
            heap.function(CapabilityId(8101)).unwrap(),
            &NoRuntime,
        )
        .unwrap();
        heap.define_own_data(
            &thinking,
            "toJSON".into(),
            heap.function(CapabilityId(8102)).unwrap(),
            &NoRuntime,
        )
        .unwrap();
        heap.define_own_data(
            &root,
            "undefinedExtension".into(),
            heap.undefined().unwrap(),
            &NoRuntime,
        )
        .unwrap();
        let projector = ara_cli::agent_message_projection::AgentMessageProjector::default();
        let mut journal =
            SessionJournal::in_memory(json!({"type":"session","version":3,"id":"h1-public"}))
                .unwrap();
        projector
            .bind_native_session(
                &mut journal,
                Arc::new(
                    ara_cli::actual_session::GraphJournalEncoder::new(values.clone(), None)
                        .unwrap(),
                ),
            )
            .unwrap();
        let owner = AgentMessage::from_graph(root.clone(), None).unwrap();
        let event = projector
            .public_event(
                &AgentEvent::MessageEnd {
                    message: owner.clone(),
                },
                false,
            )
            .unwrap();
        let public = &event["message"];
        assert!(public.get("providerPayload").is_none());
        assert!(public["content"][0].get("thinkingSignature").is_none());
        assert!(public.get("undefinedExtension").is_none());
        assert_eq!(
            public["content"][1]["arguments"]["providerPayload"],
            "business"
        );
        assert_eq!(
            public["content"][1]["arguments"]["thinkingSignature"],
            "business signature"
        );
        assert_eq!(
            runtime.calls.load(Ordering::SeqCst),
            2,
            "original message and block toJSON execute once"
        );
        assert_eq!(
            heap.export_serde(
                &heap
                    .get(&root, &"providerPayload".into(), &NoRuntime)
                    .unwrap()
            )
            .unwrap(),
            message["providerPayload"]
        );
        assert_eq!(
            heap.export_serde(
                &heap
                    .get(&thinking, &"thinkingSignature".into(), &NoRuntime)
                    .unwrap()
            )
            .unwrap(),
            "private signature"
        );
        let selected = Model {
            id: "m".into(),
            api: "a".into(),
            provider: "p".into(),
            base_url: "http://fixture".into(),
            reasoning: false,
            max_tokens: None,
            context_window: None,
            tokenizer: None,
        };
        for _ in 0..2 {
            let context = projector
                .convert_current_actual(std::slice::from_ref(&owner), &selected, &values)
                .unwrap();
            assert_eq!(context.len(), 1);
            assert!(
                context[0].same_identity(&root),
                "ordinary next request keeps the original assistant root"
            );
        }
        assert_eq!(
            runtime.calls.load(Ordering::SeqCst),
            2,
            "provider conversion does not stringify the canonical message"
        );

        let negative = heap.number(-1.0).unwrap();
        let undefined = heap.undefined().unwrap();
        runtime
            .roots
            .lock()
            .unwrap()
            .extend([negative.clone(), undefined.clone()]);
        let update = ara_ai::ActualAssistantEvent::TextStart {
            content_index: negative.clone(),
            partial: root.clone(),
        };
        assert!(update.content_index().unwrap().same_identity(&negative));
        let public = projector
            .public_event(
                &AgentEvent::ActualMessageUpdate {
                    message: owner.clone(),
                    event: update,
                },
                false,
            )
            .unwrap();
        assert_eq!(public["assistantMessageEvent"]["contentIndex"], -1);

        let programmed = heap.object().unwrap();
        heap.define_own(
            &programmed,
            "toJSON".into(),
            PropertyDescriptor::Accessor {
                get: Some(CallableTarget::Host(CapabilityId(8113))),
                set: None,
                enumerable: true,
                configurable: true,
            },
            &NoRuntime,
        )
        .unwrap();
        let update = ara_ai::ActualAssistantEvent::TextStart {
            content_index: programmed.clone(),
            partial: root.clone(),
        };
        assert!(
            update.try_printable().is_err(),
            "standalone bridge cannot substitute the current Host runtime"
        );
        let public = projector
            .public_event(
                &AgentEvent::ActualMessageUpdate {
                    message: owner.clone(),
                    event: update,
                },
                false,
            )
            .unwrap();
        assert_eq!(public["assistantMessageEvent"]["contentIndex"], -1);
        assert_eq!(
            *runtime.keys.lock().unwrap(),
            [ara_rpc::WireString::from("contentIndex")]
        );

        heap.define_own_data(
            &programmed,
            "toJSON".into(),
            heap.function(CapabilityId(8112)).unwrap(),
            &NoRuntime,
        )
        .unwrap();
        let update = ara_ai::ActualAssistantEvent::ToolcallDelta {
            content_index: programmed,
            delta: undefined.clone(),
            partial: root.clone(),
        };
        let public = projector
            .public_event(
                &AgentEvent::ActualMessageUpdate {
                    message: owner.clone(),
                    event: update,
                },
                false,
            )
            .unwrap();
        assert!(
            public["assistantMessageEvent"]
                .get("contentIndex")
                .is_none()
        );
        assert!(public["assistantMessageEvent"].get("delta").is_none());
        assert_eq!(
            *runtime.keys.lock().unwrap(),
            [
                ara_rpc::WireString::from("contentIndex"),
                ara_rpc::WireString::from("contentIndex")
            ]
        );

        let update = ara_ai::ActualAssistantEvent::TextStart {
            content_index: undefined,
            partial: root,
        };
        let public = projector
            .public_event(
                &AgentEvent::ActualMessageUpdate {
                    message: owner,
                    event: update,
                },
                false,
            )
            .unwrap();
        assert!(
            public["assistantMessageEvent"]
                .get("contentIndex")
                .is_none()
        );
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
        assert!(!is_official_anthropic(
            "https://api.anthropic.com.evil.example/v1"
        ));
        assert!(!is_official_anthropic(
            "https://api.anthropic.com@evil.example/v1"
        ));
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
        assert!(
            resolve_route(&args)
                .err()
                .unwrap()
                .to_string()
                .contains("unknown tokenizer")
        );
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
        let code = tokio::time::timeout(Duration::from_secs(5), run(args))
            .await
            .expect("observer must close")
            .unwrap();
        assert_eq!(code, 0);
        assert_eq!(server.served(), 1);
        tokio::time::timeout(
            Duration::from_secs(20),
            session_quota_recovery::actual_observations::stored_feedback_family(),
        )
        .await
        .expect("bounded original Core/SQLite quota feedback family");
        rpc_host_retry::observe_native_retry_family();
    }
}

mod task_host;
mod task_agents;

mod task_print;
