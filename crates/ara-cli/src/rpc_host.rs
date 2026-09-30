//! First production RPC host slice. Fixed OMP `rpc-mode.ts` at
//! 596f2da7101178214aa27a753529d15e6b7ad91d; complete command parity remains open.
//!
//! Input, serial commands, owned Runs and ordered stdout have distinct owners.
//! A Run keeps its original journal even if the caller stops waiting. Live
//! queries use completed event messages, never the Agent transcript lock.

use super::rpc_host_settings::{AutoCompactionPolicy, RetryPolicy};
use super::rpc_host_tools::{HostToolDefinition, ToolBridge, normalize_host_tool_definitions};
use super::rpc_host_uris::UriBridge;
use anyhow::{Context as _, Result, bail};
use ara_agent::{
    Agent, AgentConfig, AgentError, AgentEvent, AgentEventSink, AgentInput, ExecutionSnapshot, LoopHooks, QueueMode,
    RunEnd, RunReport, ToolDecision,
};
use ara_ai::{AssistantMessage, AssistantRetryRecovery, ImageContent, Message, UserBlock, UserContent, UserMessage};
use ara_discovery::LoadedSkill;
use ara_rpc::{
    WireValue,
    frame::*,
    input::{InputItem, RpcInputReader},
    writer::RpcOutput,
};
use ara_session::{BashExecutionMessage, SessionJournal, UserSkillPrompt, bash_output_meta_from_summary};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    future::pending,
    path::PathBuf,
    sync::{Arc, Mutex, RwLock, Weak},
    time::{Duration, Instant},
};

fn lexical_absolute(path: &std::path::Path) -> Result<PathBuf> {
    // Fixed path.resolve comparison does not inspect symlinks or require the
    // directory to exist. std::path::absolute supplies platform drive/root
    // semantics; fold remaining parent components without filesystem I/O.
    let absolute = std::path::absolute(path)?;
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::CurDir => {}
            component => normalized.push(component.as_os_str()),
        }
    }
    Ok(normalized)
}

pub(super) struct SessionFactory {
    pub dir: Option<PathBuf>,
    pub cwd: PathBuf,
    pub provider: super::ProviderFactory,
    pub args: super::Args,
    pub mcp_config: Option<ara_mcp::ServerConfig>,
    pub tool_context: Option<ara_tools::ToolContext>,
    pub uri_port: Option<Arc<dyn ara_tools::ContentUriPort>>,
}

impl SessionFactory {
    async fn config(
        &self,
        current: &AgentConfig,
        reset: bool,
        overlay: super::ToolOverlay,
    ) -> Result<(AgentConfig, Vec<LoadedSkill>)> {
        let setup = super::prepare_cli_setup(
            &self.args,
            &self.cwd,
            &current.model,
            &self.mcp_config,
            (!reset).then(|| current.tools.clone()),
            overlay,
        )
        .await?;
        if let (Some(context), Some(port)) = (&setup.tool_context, &self.uri_port) {
            context.set_uri_port(port.clone());
        }
        let mut config = current.clone();
        config.system_prompt = setup.system_prompt;
        config.tools = setup.tools;
        if reset {
            config.provider = self.provider.build();
            config.hooks = setup.hooks;
        }
        Ok((config, setup.skills))
    }
}

struct RpcHooks {
    base: Arc<dyn LoopHooks>,
    snapshot: Arc<RwLock<ExecutionSnapshot>>,
}

#[async_trait]
impl LoopHooks for RpcHooks {
    fn execution_snapshot(&self) -> Option<ExecutionSnapshot> {
        Some(self.snapshot.read().unwrap_or_else(|e| e.into_inner()).clone())
    }

    async fn before_tool_call(
        &self,
        call: &ara_ai::ToolCall,
        args: &ara_ai::JsonObject,
        cancel: &CancellationToken,
    ) -> ToolDecision {
        self.base.before_tool_call(call, args, cancel).await
    }

    async fn steering_inputs(&self) -> Vec<AgentInput> {
        self.base.steering_inputs().await
    }
    async fn follow_up_inputs(&self) -> Vec<AgentInput> {
        self.base.follow_up_inputs().await
    }
    async fn transform_provider_context(&self, context: ara_ai::Context, model: &ara_ai::Model) -> ara_ai::Context {
        self.base.transform_provider_context(context, model).await
    }
}

use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::mpsc,
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

enum OutputItem {
    Frame(WireValue),
    Negotiate(WireValue),
}

#[derive(Clone)]
struct Output(mpsc::UnboundedSender<OutputItem>);

fn wire(value: Value) -> WireValue {
    // serde_json's output is valid JSON; the transport preserves its JS wire
    // representation. Inbound correlation IDs never pass through serde_json.
    WireValue::parse(&value.to_string()).expect("serialized JSON")
}

impl Output {
    fn frame(&self, frame: Value) {
        self.send(OutputItem::Frame(wire(frame)));
    }

    fn send(&self, frame: OutputItem) {
        // The output owner cancels the connection on failure. There is no await
        // here: a stalled peer cannot stop input/control or journal receipts.
        let _ = self.0.send(frame);
    }

    fn response(&self, command: &Command, data: Option<Value>, error: Option<String>) {
        self.response_wire(command, data.map(wire), error, None);
    }

    fn response_wire(&self, command: &Command, data: Option<WireValue>, error: Option<String>, code: Option<&str>) {
        let mut response = json!({"type":"response", "command":command.kind, "success":error.is_none()});
        if let Some(error) = error {
            response["error"] = json!(error);
            if let Some(code) = code {
                response["code"] = json!(code);
            }
        }
        let mut response = wire(response);
        if response.get("success") == Some(&WireValue::Bool(true))
            && let Some(data) = data
        {
            response.insert("data", data);
        }
        if let (Some(id), WireValue::Object(fields)) = (&command.id, &mut response) {
            fields.push(("id".into(), id.clone()));
        }
        let negotiated =
            command.kind == "negotiate_protocol" && response.get("success") == Some(&WireValue::Bool(true));
        self.send(if negotiated { OutputItem::Negotiate(response) } else { OutputItem::Frame(response) });
    }

    fn prompt_skipped(&self, command: &Command) {
        let mut result = wire(json!({"type":"prompt_result","agentInvoked":false}));
        if let (Some(id), WireValue::Object(fields)) = (&command.id, &mut result) {
            fields.push(("id".into(), id.clone()));
        }
        self.send(OutputItem::Frame(result));
    }
}

async fn write_output<W: AsyncWrite + Unpin>(
    writer: W,
    mut rx: mpsc::UnboundedReceiver<OutputItem>,
    cancel: CancellationToken,
) -> Result<()> {
    let mut writer = RpcOutput::new(writer);
    while let Some(item) = rx.recv().await {
        let (frame, negotiate) = match item {
            OutputItem::Frame(frame) => (frame, false),
            OutputItem::Negotiate(frame) => (frame, true),
        };
        if let Err(error) = writer.write_frame(&frame).await {
            cancel.cancel();
            return Err(error).context("writing RPC output");
        }
        if negotiate {
            writer.set_protocol_version(2)?;
        }
    }
    Ok(())
}

struct Command {
    id: Option<WireValue>,
    kind: String,
    frame: WireValue,
}

impl Command {
    fn new(frame: WireValue) -> Self {
        let id = frame.get("id").cloned();
        let kind = frame.get("type").and_then(WireValue::as_string).and_then(|s| s.to_utf8().ok()).unwrap_or_default();
        Self { id, kind, frame }
    }

    fn string(&self, field: &str) -> Result<String> {
        let value = self
            .frame
            .get(field)
            .and_then(WireValue::as_string)
            .with_context(|| format!("{} requires string {field}", self.kind))?;
        value.to_utf8().with_context(|| format!("{field} contains an unsupported unpaired UTF-16 surrogate"))
    }

    fn message(&self) -> Result<Message> {
        let text = self.string("message")?;
        let images = match self.frame.get("images") {
            None => Vec::new(),
            Some(WireValue::Array(images)) => images
                .iter()
                .map(|image| {
                    let image = Command::new(image.clone());
                    Ok(ImageContent { data: image.string("data")?, mime_type: image.string("mimeType")? })
                })
                .collect::<Result<Vec<_>>>()?,
            Some(_) => bail!("images must be an array"),
        };
        let mut message = UserMessage::text(text);
        if !images.is_empty() {
            let mut content = vec![UserBlock::text(message.content.plain_text())];
            content.extend(images.into_iter().map(UserBlock::Image));
            message.content = UserContent::Blocks(content);
        }
        Ok(Message::User(message))
    }

    fn queue_mode(&self) -> Result<QueueMode> {
        match self.string("mode")?.as_str() {
            "all" => Ok(QueueMode::All),
            "one-at-a-time" => Ok(QueueMode::OneAtATime),
            _ => bail!("mode must be all or one-at-a-time"),
        }
    }
}

struct Session {
    journal: tokio::sync::Mutex<SessionJournal>,
    // Public completed messages only. A partial is kept separately and never
    // appended by get_messages while the provider is still producing it.
    messages: Mutex<Vec<Value>>,
    partial: Mutex<Option<Value>>,
    persistence_error: Mutex<Option<String>>,
    name: Mutex<Option<String>>,
    header: Value,
    file: Option<std::path::PathBuf>,
}

impl Session {
    fn new(journal: Option<SessionJournal>, header: Value, messages: &[Message]) -> Result<Arc<Self>> {
        let journal = match journal {
            Some(journal) => journal,
            None => {
                let mut journal = SessionJournal::in_memory(header.clone())?;
                for message in messages {
                    journal.append_message(message)?;
                }
                journal
            }
        };
        // Native provenance comes from the active raw branch. Reopening never
        // needs the historical Skill file, nor equality against model content.
        let public_messages = Self::public_messages(&journal);
        Ok(Arc::new(Self {
            name: Mutex::new((!journal.title().title.is_empty()).then(|| journal.title().title.clone())),
            file: journal.is_persistent().then(|| journal.path().into()),
            journal: tokio::sync::Mutex::new(journal),
            messages: Mutex::new(public_messages),
            partial: Mutex::new(None),
            persistence_error: Mutex::new(None),
            header,
        }))
    }

    fn public_messages(journal: &SessionJournal) -> Vec<Value> {
        let branch = journal.branch();
        let summary = journal.compacted_context_projection().ok().and_then(|projection| {
            projection.items.into_iter().find_map(|item| match item {
                ara_session::CompactedContextItem::Summary(summary) => Some(summary),
                _ => None,
            })
        });
        let start = summary
            .as_ref()
            .and_then(|summary| branch.iter().position(|entry| entry.id == summary.first_kept_entry_id))
            .unwrap_or(0);
        let mut messages = Vec::new();
        if let Some(summary) = summary {
            let timestamp = chrono::DateTime::parse_from_rfc3339(&summary.timestamp)
                .ok()
                .map(|timestamp| timestamp.timestamp_millis());
            messages.push(json!({"role":"compactionSummary","summary":summary.summary,
                "tokensBefore":summary.tokens_before,"method":"soft","timestamp":timestamp}));
        }
        messages.extend(branch[start..].iter().filter_map(|entry| {
            entry
                .bash_execution()
                .map(|bash| bash.event_message())
                .or_else(|| entry.skill_prompt().map(|prompt| prompt.event_message()))
                .or_else(|| entry.message().map(|message| AgentEvent::MessageEnd { message }.full()["message"].clone()))
        }));
        messages
    }
}

struct RunSink {
    session: Arc<Session>,
    output: Output,
    cancel: CancellationToken,
    connection: CancellationToken,
    terminal: Mutex<Option<Value>>,
    messages: Mutex<Vec<Value>>,
    // Exact native IDs are assigned by the retained writer, not reconstructed
    // from public timestamps or the current branch's last entry.
    entries: Mutex<Vec<(String, Message)>>,
}

fn skill_input(prompt: UserSkillPrompt) -> AgentInput {
    AgentInput { model: prompt.model_message(), provenance: Some(Arc::new(prompt.event_message())) }
}

fn input_skill(input: &AgentInput) -> Option<UserSkillPrompt> {
    let public = input.provenance.as_ref()?;
    if public["role"] != "custom"
        || public["customType"] != ara_session::SKILL_PROMPT_CUSTOM_TYPE
        || public["attribution"] != "user"
    {
        return None;
    }
    Some(UserSkillPrompt {
        content: serde_json::from_value(public.get("content")?.clone()).ok()?,
        details: public.get("details").cloned(),
        timestamp: public.get("timestamp")?.as_i64()?,
    })
}

impl RunSink {
    fn completed_message(&self, message: Value) {
        self.messages.lock().unwrap().push(message.clone());
        self.session.messages.lock().unwrap().push(message);
        *self.session.partial.lock().unwrap() = None;
    }

    fn persistence_failed(&self, error: impl std::fmt::Display) {
        *self.session.persistence_error.lock().unwrap() = Some(error.to_string());
        self.cancel.cancel();
        self.connection.cancel();
    }
}

#[async_trait]
impl AgentEventSink for RunSink {
    async fn emit(&self, event: AgentEvent) {
        let mut public = event.full();
        match &event {
            AgentEvent::MessageEnd { message } => {
                let mut journal = self.session.journal.lock().await;
                match journal.append_message(message) {
                    Ok(id) => self.entries.lock().unwrap().push((id, message.clone())),
                    Err(error) => {
                        // The loop awaits this sink before starting tools.
                        self.persistence_failed(error);
                    }
                }
                drop(journal);
                self.completed_message(public["message"].clone());
            }
            AgentEvent::MessageStart { message: Message::Assistant(_) } | AgentEvent::MessageUpdate { .. } => {
                *self.session.partial.lock().unwrap() = Some(public["message"].clone());
            }
            AgentEvent::AgentEnd { .. } => {
                *self.session.partial.lock().unwrap() = None;
                public["messages"] = json!(*self.messages.lock().unwrap());
                // Fixed AgentSession defers the wire terminal until the prompt
                // unwinds. The client may immediately start its next Run.
                *self.terminal.lock().unwrap() = Some(public);
                return;
            }
            _ => {}
        }
        self.output.frame(public);
    }

    async fn emit_input(&self, input: AgentInput) {
        let Some(prompt) = input_skill(&input) else {
            self.emit(AgentEvent::MessageStart { message: input.model.clone() }).await;
            self.emit(AgentEvent::MessageEnd { message: input.model }).await;
            return;
        };
        let public = prompt.event_message();
        self.output.frame(json!({"type":"message_start","message":public}));
        if let Err(error) = self.session.journal.lock().await.append_skill_prompt(&prompt) {
            self.persistence_failed(error);
        }
        self.completed_message(public.clone());
        self.output.frame(json!({"type":"message_end","message":public}));
    }
}

struct ActiveRun {
    cancel: CancellationToken,
    task: JoinHandle<std::result::Result<Option<RunReport>, AgentError>>,
    command: Command,
    sink: Arc<RunSink>,
}

struct PendingRetryError {
    entry_id: String,
    message: AssistantMessage,
    attempt: usize,
    recovery: String,
    note: String,
}

struct RetrySaga {
    session: Arc<Session>,
    generation: u64,
    command: Command,
    attempt: usize,
    pending: Vec<PendingRetryError>,
    deadline: Option<Instant>,
    cancel: CancellationToken,
    visible: bool,
    expected_messages: Vec<Message>,
}

#[derive(Clone)]
enum BashDestination {
    Current,
    Detached { parent: Option<String> },
    Branch { parent: Option<String> },
}

// Host-owned destinations are shared by jobs started in one ownership scope.
// Completions and transitions mutate them only in the serial command owner.
struct BashTarget {
    session: Arc<Session>,
    destination: BashDestination,
}

struct BashJob {
    command: Command,
    text: String,
    target: Arc<Mutex<BashTarget>>,
    cancel: CancellationToken,
    task: JoinHandle<()>,
}

type BashCompletion = (
    u64,
    std::result::Result<std::result::Result<ara_tools::bash::BashResult, ara_agent::ToolError>, tokio::task::JoinError>,
);

struct BashDispatcher {
    current: Mutex<Arc<Mutex<BashTarget>>>,
    jobs: Mutex<std::collections::HashMap<u64, BashJob>>,
    next_id: std::sync::atomic::AtomicU64,
    done: mpsc::UnboundedSender<BashCompletion>,
    output: Output,
    connection: CancellationToken,
    cwd: PathBuf,
}

impl BashDispatcher {
    // Fixed RpcInputDispatcher starts bash as soon as stdin dispatches it,
    // independently of the ordinary serial command tail. The process task
    // cannot write a journal: the Host owns every completion and transition.
    fn dispatch(&self, command: Command) {
        let text = match command.string("command") {
            Ok(text) => text,
            Err(error) => {
                self.output.response(&command, None, Some(error.to_string()));
                return;
            }
        };
        let target = self.current.lock().unwrap().clone();
        let cancel = self.connection.child_token();
        let process_cancel = cancel.clone();
        let context = ara_tools::ToolContext::new(self.cwd.clone());
        let args = json!({"command":text}).as_object().unwrap().clone();
        let id = self.next_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let done = self.done.clone();
        // Insert and install the supervisor under one lock. A fast completion
        // therefore cannot outrun its ownership record.
        let mut jobs = self.jobs.lock().unwrap();
        let process = tokio::spawn(async move {
            ara_tools::bash::execute_bash(&context, &args, process_cancel, Arc::new(|_| {})).await
        });
        let task = tokio::spawn(async move {
            let result = process.await;
            let _ = done.send((id, result));
        });
        jobs.insert(id, BashJob { command, text, target, cancel, task });
    }

    fn abort(&self) {
        for job in self.jobs.lock().unwrap().values() {
            job.cancel.cancel();
        }
    }

    fn is_empty(&self) -> bool {
        self.jobs.lock().unwrap().is_empty()
    }
}

struct PendingBash {
    target: Arc<Mutex<BashTarget>>,
    message: BashExecutionMessage,
}

struct Host {
    agent: Arc<Agent>,
    session: Arc<Session>,
    config: AgentConfig,
    skills: Vec<LoadedSkill>,
    max_time: Option<f64>,
    output: Output,
    connection: CancellationToken,
    active: Option<ActiveRun>,
    sessions: SessionFactory,
    tool_bridge: Arc<ToolBridge>,
    uri_bridge: Arc<UriBridge>,
    host_tools: Vec<HostToolDefinition>,
    snapshot: Arc<RwLock<ExecutionSnapshot>>,
    // Deliberate abort keeps queued input visible but suppresses autonomous
    // resumption. A new explicit prompt/queue command permits another Run.
    drain_queues: bool,
    bash_target: Arc<Mutex<BashTarget>>,
    bash_targets: Vec<Weak<Mutex<BashTarget>>>,
    bash_dispatcher: Arc<BashDispatcher>,
    pending_bash: Vec<PendingBash>,
    bash_error: Option<String>,
    compaction_policy: AutoCompactionPolicy,
    is_compacting: bool,
    auto_compaction_pending: bool,
    // A failed/no-progress pass is not billed again for the same source view.
    auto_compaction_checked: Option<(String, Option<String>, usize)>,
    retry_policy: RetryPolicy,
    retry: Option<RetrySaga>,
    prompt_generation: u64,
}

fn same_session_file(left: &Session, right: &Session) -> bool {
    if left.header["id"] != right.header["id"] {
        return false;
    }
    match (&left.file, &right.file) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            std::fs::canonicalize(left).ok().zip(std::fs::canonicalize(right).ok()).is_some_and(|(a, b)| a == b)
                || lexical_absolute(left).ok().zip(lexical_absolute(right).ok()).is_some_and(|(a, b)| a == b)
        }
        _ => false,
    }
}

fn bash_result_value(result: &ara_tools::bash::BashResult) -> Value {
    let mut value = json!({"output":result.output_with_status_notice(),"cancelled":result.cancelled,"truncated":result.truncated,
        "totalLines":result.total_lines,"totalBytes":result.total_bytes,
        "outputLines":result.output_lines,"outputBytes":result.output_bytes});
    if let Some(exit) = result.exit_code {
        value["exitCode"] = json!(exit);
    }
    if result.timed_out {
        value["timedOut"] = json!(true);
    }
    if let Some(cwd) = &result.working_dir {
        value["workingDir"] = json!(cwd);
    }
    value
}

impl Host {
    async fn finish_retry(&mut self, success: Option<&AssistantMessage>, error: Option<String>, supersede: bool) {
        let Some(saga) = self.retry.take() else { return };
        saga.cancel.cancel();
        let mut final_error = error;
        let mut updates = Vec::new();
        if success.is_some() || supersede {
            let records = saga.pending.iter().map(|pending| {
                let recovery = AssistantRetryRecovery {
                    kind: "auto-retry".into(),
                    status: if success.is_some() { "recovered" } else { "superseded" }.into(),
                    attempt: pending.attempt,
                    recovery: pending.recovery.clone(),
                    note: pending.note.clone(),
                    recovered_at: success.map(|_| chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
                    superseded_by: success.map(|message| {
                        let mut value = json!({"timestamp":message.timestamp,"provider":message.provider,"model":message.model});
                        if let Some(id) = &message.response_id { value["responseId"] = json!(id); }
                        value
                    }),
                };
                (pending.entry_id.clone(), recovery)
            }).collect::<Vec<_>>();
            let persisted = saga.session.journal.lock().await.update_retry_recovery(&records);
            match persisted {
                Ok(()) => {
                    let mut messages = self.agent.messages().await;
                    let mut public = saga.session.messages.lock().unwrap();
                    for (pending, (entry_id, recovery)) in saga.pending.iter().zip(&records) {
                        for message in &mut messages {
                            if let Message::Assistant(message) = message
                                && message == &pending.message
                            {
                                message.retry_recovery = Some(recovery.clone());
                            }
                        }
                        let failed = AgentEvent::MessageEnd { message: Message::Assistant(pending.message.clone()) }
                            .full()["message"]
                            .clone();
                        for message in public.iter_mut().filter(|message| **message == failed) {
                            message["retryRecovery"] = json!(recovery);
                        }
                        updates.push(json!({"entryId":entry_id,"persistenceKey":super::rpc_host_retry::persistence_key(&pending.message),
                            "note":recovery.note,"retryRecovery":recovery}));
                    }
                    drop(public);
                    if let Err(error) = self.agent.replace_idle_messages(messages) {
                        final_error = Some(format!("Retry recovery projection failed: {error}"));
                        *saga.session.persistence_error.lock().unwrap() = final_error.clone();
                        self.connection.cancel();
                    }
                }
                Err(error) => {
                    final_error = Some(format!("Retry recovery persistence failed: {error}"));
                    *saga.session.persistence_error.lock().unwrap() = final_error.clone();
                    self.connection.cancel();
                }
            }
        }
        let mut event = json!({"type":"auto_retry_end","success":success.is_some() && final_error.is_none(),"attempt":saga.attempt});
        if let Some(error) = final_error {
            event["finalError"] = json!(error);
        }
        if success.is_some() || supersede {
            event["retryErrors"] = json!(updates);
        }
        self.output.frame(event);
    }

    async fn abort_retry(&mut self) {
        if self.retry.as_ref().is_some_and(|retry| retry.deadline.is_some()) {
            self.finish_retry(None, Some("Retry cancelled".into()), false).await;
            self.drain_queues = false;
        } else if let Some(retry) = &mut self.retry {
            // Fixed abortRetry resolves its wait promise; an already scheduled
            // Agent continuation is owned by the Run and is not aborted here.
            retry.visible = false;
        }
    }

    async fn begin_retry(&mut self, active: &ActiveRun, message: &AssistantMessage) -> Result<bool> {
        use super::rpc_host_retry::{RetryDisposition, backoff_ms, disposition};
        let class = ara_ai::retry_classification::classify_retry(message, &self.config.model.api);
        if active.cancel.is_cancelled()
            || self.connection.is_cancelled()
            || !Arc::ptr_eq(&active.sink.session, &self.session)
            || !self.retry_policy.enabled()
        {
            return Ok(false);
        }
        let mut messages = self.agent.messages().await;
        let Some(disposition) = disposition(message, &messages, &class) else { return Ok(false) };
        let entry_id = active
            .sink
            .entries
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find_map(|(id, candidate)| (candidate.as_assistant() == Some(message)).then(|| id.clone()))
            .context("failed retry message has no durable native entry ID")?;
        let previous_attempt = self.retry.as_ref().map_or(0, |retry| retry.attempt);
        let attempt = previous_attempt.saturating_add(1);
        if attempt as f64 > self.retry_policy.max_retries() {
            // The terminal failure remains raw; only prior retry receipts are
            // marked superseded, as fixed turn-recovery.ts:2310-2325.
            if self.retry.is_none() {
                self.retry = Some(RetrySaga {
                    session: self.session.clone(),
                    generation: self.prompt_generation,
                    command: Command {
                        id: active.command.id.clone(),
                        kind: active.command.kind.clone(),
                        frame: active.command.frame.clone(),
                    },
                    attempt: previous_attempt,
                    pending: Vec::new(),
                    deadline: None,
                    cancel: self.connection.child_token(),
                    visible: true,
                    expected_messages: messages.clone(),
                });
            }
            self.finish_retry(None, message.error_message.clone(), true).await;
            return Ok(false);
        }
        let randomness = uuid::Uuid::new_v4();
        let random = u64::from_le_bytes(randomness.as_bytes()[..8].try_into().unwrap()) as f64 / u64::MAX as f64;
        let mut delay_ms =
            if class.stale_responses { 0.0 } else { backoff_ms(self.retry_policy.base_delay_ms(), attempt, random) };
        // Fixed turn-recovery.ts:2171-2174,2303-2306: a same-route hint
        // may raise the backoff but must never shorten it. Stale Responses
        // recovery keeps its independent zero-delay branch.
        if !class.stale_responses
            && let Some(wait_ms) = class.wait_ms
            && wait_ms > delay_ms
        {
            delay_ms = wait_ms;
        }
        if !delay_ms.is_finite()
            || delay_ms < 0.0
            || (self.retry_policy.max_delay_ms() > 0.0 && delay_ms > self.retry_policy.max_delay_ms())
        {
            if let Some(retry) = &mut self.retry {
                retry.attempt = attempt;
            } else {
                self.retry = Some(RetrySaga {
                    session: self.session.clone(),
                    generation: self.prompt_generation,
                    command: Command {
                        id: active.command.id.clone(),
                        kind: active.command.kind.clone(),
                        frame: active.command.frame.clone(),
                    },
                    attempt,
                    pending: Vec::new(),
                    deadline: None,
                    cancel: self.connection.child_token(),
                    visible: true,
                    expected_messages: Vec::new(),
                });
            }
            self.finish_retry(
                None,
                Some(format!(
                    "Provider requested {}ms wait, exceeds retry.maxDelayMs ({}ms). Original error: {}",
                    delay_ms.ceil(),
                    self.retry_policy.max_delay_ms(),
                    message.error_message.as_deref().unwrap_or("Unknown error")
                )),
                false,
            )
            .await;
            return Ok(false);
        }
        let duration = Duration::try_from_secs_f64(delay_ms / 1000.0).context("retry delay is not representable")?;
        // Fixed scheduleAgentContinue defers by one millisecond after backoff;
        // a zero wait still gives the serial reader a command boundary.
        let deadline = Instant::now()
            .checked_add(duration.saturating_add(Duration::from_millis(1)))
            .context("retry deadline is not representable")?;
        self.agent.replace_idle_messages(messages.clone())?;
        let saga = self.retry.get_or_insert_with(|| RetrySaga {
            session: self.session.clone(),
            generation: self.prompt_generation,
            command: Command {
                id: active.command.id.clone(),
                kind: active.command.kind.clone(),
                frame: active.command.frame.clone(),
            },
            attempt: 0,
            pending: Vec::new(),
            deadline: None,
            cancel: self.connection.child_token(),
            visible: true,
            expected_messages: Vec::new(),
        });
        if !Arc::ptr_eq(&saga.session, &self.session) || saga.generation != self.prompt_generation {
            bail!("retry owner no longer matches this Session generation");
        }
        let recovery = if class.usage_limit && delay_ms > 0.0 { "wait" } else { "plain" };
        let note = if class.usage_limit {
            if recovery == "wait" { "rate-limited; waited; retried" } else { "rate-limited; retried" }
        } else {
            "error; retried"
        };
        if !saga.pending.iter().any(|pending| pending.entry_id == entry_id) {
            saga.pending.push(PendingRetryError {
                entry_id,
                message: message.clone(),
                attempt,
                recovery: recovery.into(),
                note: note.into(),
            });
        }
        saga.attempt = attempt;
        saga.visible = true;
        saga.deadline = Some(deadline);
        self.output.frame(json!({"type":"auto_retry_start","attempt":attempt,"maxAttempts":self.retry_policy.max_retries(),
            "delayMs":delay_ms,"errorMessage":message.error_message.as_deref().unwrap_or("Unknown error"),"errorId":class.error_id}));
        if disposition == RetryDisposition::RemoveFailed {
            if messages.last().and_then(Message::as_assistant) != Some(message) {
                bail!("failed retry assistant is no longer the exact active tail");
            }
            messages.pop();
            self.agent.replace_idle_messages(messages.clone())?;
            let failed =
                AgentEvent::MessageEnd { message: Message::Assistant(message.clone()) }.full()["message"].clone();
            let mut public = self.session.messages.lock().unwrap();
            if let Some(index) = public.iter().rposition(|candidate| candidate == &failed) {
                public.remove(index);
            }
        }
        saga.expected_messages = messages;
        if class.stale_responses {
            self.config.provider = self.sessions.provider.build();
        }
        self.auto_compaction_pending = false;
        Ok(true)
    }

    async fn resume_retry(&mut self) {
        let Some(saga) = &self.retry else { return };
        if saga.cancel.is_cancelled()
            || saga.generation != self.prompt_generation
            || !Arc::ptr_eq(&saga.session, &self.session)
            || self.connection.is_cancelled()
        {
            self.finish_retry(None, Some("Retry cancelled".into()), false).await;
            return;
        }
        let messages = self.agent.messages().await;
        if messages != saga.expected_messages {
            self.finish_retry(
                None,
                Some("Retry continuation failed locally: the active context changed during backoff".into()),
                false,
            )
            .await;
            self.drain_queues = false;
            return;
        }
        let saga = self.retry.as_mut().unwrap();
        saga.deadline = None;
        let command =
            Command { id: saga.command.id.clone(), kind: saga.command.kind.clone(), frame: saga.command.frame.clone() };
        if let Err(error) = self.start(None, command) {
            self.finish_retry(None, Some(format!("Retry continuation failed locally: {error}")), false).await;
            self.drain_queues = false;
        }
    }

    async fn compact(&mut self, focus: Option<&str>) -> Result<Value> {
        use ara_agent::compaction::{SummarySource, select_whole_turn_cut, summarize_sources_with_instructions};
        use ara_agent::tokenizer::{MessageCountOptions, count_messages};
        if self.session.persistence_error.lock().unwrap().is_some() {
            bail!("session persistence failed; restart from the journal before compacting");
        }
        if focus.is_some_and(|focus| focus.len() > 1_000_000) {
            bail!("compaction instructions exceed the summary input limit");
        }
        // Compaction keeps Agent queues; explicit abort normally suppresses
        // autonomous draining, so restore the previous intent after cleanup.
        let drain = self.drain_queues;
        self.is_compacting = true;
        self.abort().await;
        let result = async {
            if self.connection.is_cancelled() {
                bail!("Request was aborted");
            }
            let current = self.agent.messages().await;
            // Check recovery/admission before billing or changing the journal.
            self.agent.replace_idle_messages(current.clone())?;
            let snapshot = self.session.journal.lock().await.projected_compaction_snapshot()?;
            let previous = snapshot.previous_summary.as_ref().map(|summary| summary.summary.as_str());
            let sources = snapshot
                .messages
                .iter()
                .map(|message| SummarySource { entry_id: message.entry_id.as_str(), message: &message.message })
                .collect::<Vec<_>>();
            let cut = select_whole_turn_cut(&sources, self.sessions.args.compact_keep_tokens, previous)
                .map_err(|error| anyhow::anyhow!("{error}"))?
                .context("No earlier completed turn can be compacted with the current keep-token budget")?;
            let tokens_before = count_messages(&current, MessageCountOptions::default()) as u64;
            let seconds = self.max_time.unwrap_or(120.0).clamp(0.0, 120.0);
            let deadline = Instant::now() + Duration::from_secs_f64(seconds);
            let max_tokens = self.config.max_tokens.unwrap_or(13_107).min(13_107);
            let accepted = summarize_sources_with_instructions(
                &sources[..cut.candidate.first_kept_index],
                previous,
                focus,
                &self.config.model,
                self.config.provider.as_ref(),
                max_tokens,
                deadline,
                &self.connection,
            )
            .await
            .map_err(|error| anyhow::anyhow!("Compaction summary failed: {error}"))?;
            let mut journal = self.session.journal.lock().await;
            let old_leaf = journal.leaf_id().map(str::to_owned);
            if let Err(error) = journal.commit_projected_compaction(
                &snapshot,
                &accepted.text,
                &cut.candidate.first_kept_entry_id,
                &accepted.window_source_entry_ids,
                tokens_before,
            ) {
                if matches!(error, ara_session::CompactionCommitError::Storage(_)) {
                    *self.session.persistence_error.lock().unwrap() = Some(error.to_string());
                    self.connection.cancel();
                }
                return Err(error.into());
            }
            if let Err(error) = self.agent.replace_idle_messages(journal.model_context()) {
                *self.session.persistence_error.lock().unwrap() = Some(error.to_string());
                self.connection.cancel();
                return Err(error.into());
            }
            *self.session.messages.lock().unwrap() = Session::public_messages(&journal);
            // Reader-only Bash remains independent during the summary. Hold
            // admission while changing its original owner's branch target.
            let mut dispatcher_target = self.bash_dispatcher.current.lock().unwrap();
            for target in self.bash_targets.iter().filter_map(Weak::upgrade) {
                let mut target = target.lock().unwrap();
                if Arc::ptr_eq(&target.session, &self.session) && matches!(target.destination, BashDestination::Current)
                {
                    target.destination = BashDestination::Branch { parent: old_leaf.clone() };
                }
            }
            let target = Arc::new(Mutex::new(BashTarget {
                session: self.session.clone(),
                destination: BashDestination::Current,
            }));
            *dispatcher_target = target.clone();
            self.bash_targets.retain(|target| target.strong_count() != 0);
            self.bash_targets.push(Arc::downgrade(&target));
            self.bash_target = target;
            // The rewritten context invalidates stateful provider replay.
            self.config.provider = self.sessions.provider.build();
            Ok(json!({"summary":accepted.text,"firstKeptEntryId":cut.candidate.first_kept_entry_id,
                "tokensBefore":tokens_before}))
        }
        .await;
        self.is_compacting = false;
        self.auto_compaction_pending = false;
        self.drain_queues = drain && !self.connection.is_cancelled();
        result
    }

    async fn maybe_auto_compact(&mut self, pending: &[Message]) {
        use ara_agent::tokenizer::{MessageCountOptions, count_messages};
        let threshold = self.sessions.args.compact_threshold;
        if !self.compaction_policy.enabled()
            || threshold == 0
            || self.active.is_some()
            || self.retry.is_some()
            || self.connection.is_cancelled()
        {
            return;
        }
        let messages = self.agent.messages().await;
        // Fixed threshold uses billed context floored by the stored estimate.
        // The projected transcript contains only kept/post-summary messages.
        let billed = messages
            .iter()
            .rev()
            .find_map(|message| match message {
                Message::Assistant(message)
                    if message.model == self.config.model.id
                        && message.provider == self.config.model.provider
                        && matches!(message.stop_reason, ara_ai::StopReason::Stop | ara_ai::StopReason::Length) =>
                {
                    Some(
                        message
                            .usage
                            .input
                            .unwrap_or(0)
                            .saturating_add(message.usage.cache_read.unwrap_or(0))
                            .saturating_add(message.usage.cache_write.unwrap_or(0)),
                    )
                }
                Message::Assistant(_) => Some(0),
                _ => None,
            })
            .unwrap_or(0);
        let stored = count_messages(&messages, MessageCountOptions::default());
        let pending_tokens = count_messages(pending, MessageCountOptions::default());
        let tokens = stored.max(usize::try_from(billed).unwrap_or(usize::MAX)).saturating_add(pending_tokens);
        if tokens <= threshold {
            return;
        }
        let key = {
            let journal = self.session.journal.lock().await;
            (journal.session_id().to_owned(), journal.leaf_id().map(str::to_owned), tokens)
        };
        if self.auto_compaction_checked.as_ref() == Some(&key) {
            return;
        }
        self.auto_compaction_checked = Some(key);
        self.output.frame(json!({"type":"auto_compaction_start","reason":"threshold","action":"context-full"}));
        let result = self.compact(None).await;
        let mut event = json!({"type":"auto_compaction_end","action":"context-full",
            "aborted":self.connection.is_cancelled(),"willRetry":false});
        match result {
            Ok(result) => event["result"] = result,
            Err(error) => event["errorMessage"] = json!(super::sanitize_text(&error.to_string())),
        }
        self.output.frame(event);
    }

    async fn append_bash(&mut self, pending: PendingBash) -> Result<()> {
        let (session, destination) = {
            let target = pending.target.lock().unwrap();
            (target.session.clone(), target.destination.clone())
        };
        match destination {
            BashDestination::Current => {
                if !Arc::ptr_eq(&session, &self.session) {
                    bail!("Bash current destination no longer owns the active Session");
                }
                let model_message = pending.message.model_message();
                if let Some(message) = &model_message {
                    // Reject an unresolved tool tail before a User projection
                    // can hide it in either live or persisted model context.
                    self.agent.append_idle_message(message.clone())?;
                }
                let mut journal = session.journal.lock().await;
                journal.append_bash_execution(&pending.message)?;
                session.messages.lock().unwrap().push(pending.message.event_message());
                if let Some(message) = model_message
                    && let Some(retry) = &mut self.retry
                    && retry.generation == self.prompt_generation
                    && Arc::ptr_eq(&retry.session, &session)
                {
                    // A durable, independently accepted Bash may append while
                    // retry waits. Extend only this known owner's expected view;
                    // arbitrary replacement still fails the continuation check.
                    retry.expected_messages.push(message);
                }
            }
            BashDestination::Detached { parent } | BashDestination::Branch { parent } => {
                let mut journal = session.journal.lock().await;
                let id = journal.append_bash_execution_to_branch(&pending.message, parent.as_deref())?;
                let mut target = pending.target.lock().unwrap();
                match &mut target.destination {
                    BashDestination::Detached { parent } | BashDestination::Branch { parent } => *parent = Some(id),
                    BashDestination::Current => unreachable!("serial Host owns destination transitions"),
                }
            }
        }
        Ok(())
    }

    fn bash_persistence_failed(&mut self, target: &Arc<Mutex<BashTarget>>, error: &anyhow::Error) {
        let session = target.lock().unwrap().session.clone();
        let message = format!("Bash receipt could not be recorded; effects may have occurred: {error:#}");
        *session.persistence_error.lock().unwrap() = Some(message.clone());
        self.bash_error = Some(message.clone());
        eprintln!("ara: {message}");
        self.connection.cancel();
    }

    async fn flush_pending_bash(&mut self) {
        if self.active.is_some() {
            return;
        }
        for pending in std::mem::take(&mut self.pending_bash) {
            let target = pending.target.clone();
            if let Err(error) = self.append_bash(pending).await {
                self.bash_persistence_failed(&target, &error);
            }
        }
    }

    async fn completed_bash(&mut self, (id, result): BashCompletion) {
        let job = self.bash_dispatcher.jobs.lock().unwrap().remove(&id).expect("tracked Bash completion");
        let _ = job.task.await;
        let result = match result {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => {
                self.output.response(&job.command, None, Some(error.0));
                return;
            }
            Err(error) => {
                self.output.response(
                    &job.command,
                    None,
                    Some(format!("Bash task failed; effects may be unknown: {error}")),
                );
                return;
            }
        };
        let data = bash_result_value(&result);
        let message = BashExecutionMessage {
            command: job.text,
            output: result.output_with_status_notice(),
            exit_code: result.exit_code,
            cancelled: result.cancelled,
            truncated: result.truncated,
            meta: bash_output_meta_from_summary(&data),
            timestamp: chrono::Utc::now().timestamp_millis(),
            exclude_from_context: None,
        };
        let defer = {
            let target = job.target.lock().unwrap();
            matches!(target.destination, BashDestination::Current)
                && Arc::ptr_eq(&target.session, &self.session)
                && self.active.is_some()
        };
        let pending = PendingBash { target: job.target.clone(), message };
        if defer {
            self.pending_bash.push(pending);
        } else if let Err(error) = self.append_bash(pending).await {
            self.output.response(&job.command, None, Some(error.to_string()));
            self.bash_persistence_failed(&job.target, &error);
            return;
        }
        self.output.response(&job.command, Some(data), None);
    }

    fn overlay(&self, definitions: &[HostToolDefinition], active_names: &[String]) -> super::ToolOverlay {
        let active = active_names
            .iter()
            .filter_map(|name| definitions.iter().find(|tool| tool.name == *name).cloned())
            .collect::<Vec<_>>();
        super::ToolOverlay {
            tools: self.tool_bridge.adapters(&active),
            labels: active.into_iter().map(|tool| (tool.name, tool.label)).collect(),
        }
    }

    fn active_host_names(&self) -> Vec<String> {
        self.config
            .tools
            .iter()
            .map(|tool| tool.definition().name.clone())
            .filter(|name| self.host_tools.iter().any(|tool| tool.name == *name))
            .collect()
    }

    fn base_config(&self) -> AgentConfig {
        let mut config = self.config.clone();
        config.tools.retain(|tool| !self.host_tools.iter().any(|host| host.name == tool.definition().name));
        config
    }

    async fn session_config(&self, reset: bool) -> Result<(AgentConfig, Vec<LoadedSkill>)> {
        let overlay = self.overlay(&self.host_tools, &self.active_host_names());
        let (mut config, skills) = self.sessions.config(&self.base_config(), reset, overlay).await?;
        if reset {
            config.hooks = Arc::new(RpcHooks { base: config.hooks.clone(), snapshot: self.snapshot.clone() });
        }
        Ok((config, skills))
    }

    fn publish_snapshot(&self, config: &AgentConfig) {
        *self.snapshot.write().unwrap_or_else(|e| e.into_inner()) =
            ExecutionSnapshot { tools: config.tools.clone(), system_prompt: config.system_prompt.clone() };
    }

    async fn set_host_tools(&mut self, definitions: Vec<HostToolDefinition>) -> Result<Vec<String>> {
        let names = definitions.iter().map(|tool| tool.name.clone()).collect::<Vec<_>>();
        let unique = names.iter().collect::<std::collections::HashSet<_>>();
        if unique.len() != names.len() {
            bail!("RPC host tool names must be unique");
        }
        let base = self.base_config();
        for name in &names {
            if super::TOOL_NAMES.contains(&name.as_str())
                || base.tools.iter().any(|tool| tool.definition().name == *name)
            {
                bail!("RPC host tool \"{name}\" conflicts with an existing tool");
            }
        }
        let mut active = self.active_host_names().into_iter().filter(|name| unique.contains(name)).collect::<Vec<_>>();
        active.extend(
            definitions
                .iter()
                .filter(|tool| !tool.hidden && !self.host_tools.iter().any(|previous| previous.name == tool.name))
                .map(|tool| tool.name.clone()),
        );
        let overlay = self.overlay(&definitions, &active);
        // Prepare the complete prompt and adapters before publishing either.
        // Pending calls retain their old adapter and original Session sink.
        let (config, skills) = self.sessions.config(&base, false, overlay).await?;
        self.publish_snapshot(&config);
        self.config = config;
        self.skills = skills;
        self.host_tools = definitions;
        Ok(names)
    }

    fn available_commands(&self) -> Vec<Value> {
        // Fixed available-commands.ts:59-68 advertises the registered Skill
        // snapshot, including hidden entries. Body loading belongs to prompt
        // admission; a metadata query must not reopen the source file.
        self.skills
            .iter()
            .map(|skill| {
                let description = if skill.description.is_empty() {
                    format!("Run {} skill", skill.name)
                } else {
                    skill.description.clone()
                };
                json!({"name":format!("skill:{}", skill.name),"description":description,
                    "input":{"hint":"arguments"},"source":"skill"})
            })
            .collect()
    }

    fn emit_available_commands(&self) {
        self.output.frame(json!({"type":"available_commands_update","commands":self.available_commands()}));
    }

    async fn adopt(
        &mut self,
        journal: Option<SessionJournal>,
        header: Value,
        messages: Vec<Message>,
        config: AgentConfig,
        skills: Vec<LoadedSkill>,
    ) -> Result<()> {
        // Preparation is complete and the old owned Run is joined. Commit the
        // replacement once; each settled RunSink retains its original journal.
        let previous_leaf = self.session.journal.lock().await.leaf_id().map(str::to_owned);
        let proposed = Session::new(journal, header, &messages)?;
        let new_leaf = proposed.journal.lock().await.leaf_id().map(str::to_owned);
        let targets = self.bash_targets.iter().filter_map(Weak::upgrade).collect::<Vec<_>>();
        // A still-owned Bash may retain the same file that switch just opened.
        // Reuse its journal lock instead of leaving a stale snapshot writer.
        let retained = targets.iter().find_map(|target| {
            let session = target.lock().unwrap().session.clone();
            same_session_file(&session, &proposed).then_some(session)
        });
        let session = if let Some(retained) = retained {
            std::mem::swap(&mut *retained.journal.lock().await, &mut *proposed.journal.lock().await);
            *retained.messages.lock().unwrap() = proposed.messages.lock().unwrap().clone();
            *retained.name.lock().unwrap() = proposed.name.lock().unwrap().clone();
            *retained.partial.lock().unwrap() = None;
            *retained.persistence_error.lock().unwrap() = None;
            retained
        } else {
            proposed
        };
        for target in &targets {
            let mut target = target.lock().unwrap();
            target.destination = match &target.destination {
                BashDestination::Current if Arc::ptr_eq(&target.session, &self.session) => {
                    if Arc::ptr_eq(&target.session, &session) {
                        if previous_leaf == new_leaf {
                            BashDestination::Current
                        } else {
                            BashDestination::Branch { parent: previous_leaf.clone() }
                        }
                    } else {
                        BashDestination::Detached { parent: previous_leaf.clone() }
                    }
                }
                BashDestination::Detached { parent } | BashDestination::Branch { parent }
                    if Arc::ptr_eq(&target.session, &session) =>
                {
                    BashDestination::Branch { parent: parent.clone() }
                }
                destination => destination.clone(),
            };
        }
        let bash_target =
            Arc::new(Mutex::new(BashTarget { session: session.clone(), destination: BashDestination::Current }));
        self.bash_targets.retain(|target| target.strong_count() != 0);
        self.bash_targets.push(Arc::downgrade(&bash_target));
        *self.bash_dispatcher.current.lock().unwrap() = bash_target.clone();
        self.bash_target = bash_target;
        let agent = Agent::new(config.clone(), messages);
        agent.set_steering_mode(self.agent.steering_mode());
        agent.set_follow_up_mode(self.agent.follow_up_mode());
        self.publish_snapshot(&config);
        self.agent = agent;
        self.session = session;
        self.config = config;
        self.skills = skills;
        self.drain_queues = false;
        Ok(())
    }

    async fn new_session(&mut self, parent: Option<&str>) -> Result<()> {
        self.abort().await;
        let (config, skills) = self.session_config(true).await?;
        let mut journal = self
            .sessions
            .dir
            .as_ref()
            .map(|dir| SessionJournal::create_with_parent(dir, &self.sessions.cwd, parent))
            .transpose()?;
        if let Some(journal) = journal.as_mut() {
            journal.append_model_change(&format!("{}/{}", config.model.provider, config.model.id))?;
            // An explicit new Session must survive restart even with no prompt.
            journal.materialize()?;
        }
        let header = journal
            .as_ref()
            .map(|journal| journal.header().clone())
            .unwrap_or_else(|| super::ephemeral_header(&self.sessions.cwd, parent));
        self.adopt(journal, header, Vec::new(), config, skills).await?;
        Ok(())
    }

    async fn switch_session(&mut self, path: PathBuf) -> Result<bool> {
        self.abort().await;
        if self.sessions.dir.is_none() {
            bail!("--no-session cannot load a disk Session; restart without --no-session");
        }
        // Join before opening, including a current lazy journal which was not
        // on disk before its accepted Run's final cancellation receipt.
        let mut journal = SessionJournal::open(&path).with_context(|| format!("opening session {}", path.display()))?;
        if let Some(cwd) = journal.header().get("cwd").and_then(Value::as_str)
            && lexical_absolute(std::path::Path::new(cwd))? != lexical_absolute(&self.sessions.cwd)?
        {
            // Fixed RPC supplies no onCwdChange callback: this is a cancelled
            // switch, including an inaccessible recorded cwd, not an adoption.
            return Ok(true);
        }
        let same_file = self.session.file.as_ref().is_some_and(|current| {
            std::fs::canonicalize(current).ok().zip(std::fs::canonicalize(&path).ok()).is_some_and(|(a, b)| a == b)
        });
        let messages = journal.model_context();
        let unchanged = same_file
            && journal.header()["id"] == self.session.header["id"]
            && replay_messages_equal(&messages, &self.agent.messages().await);
        let (config, skills) = self.session_config(!unchanged).await?;
        // The launch-selected route is currently the only available model.
        // Keep unavailable saved selections intact, as fixed switch's fallback
        // does; role/catalog/thinking restoration remains a mapped WIP gap.
        super::recover_session(&mut journal)?;
        let messages = journal.model_context();
        let header = journal.header().clone();
        self.adopt(Some(journal), header, messages, config, skills).await?;
        Ok(false)
    }

    async fn branch(&mut self, entry_id: &str) -> Result<String> {
        // Select raw identity before cancellation; empty/image-only ordinary
        // users are valid even though the branch picker omits their empty text.
        let (parent, text) = {
            let journal = self.session.journal.lock().await;
            let entry = journal
                .entries()
                .iter()
                .rev()
                .find(|entry| entry.id == entry_id)
                .filter(|entry| entry.kind == "message" && entry.raw["message"]["role"] == "user")
                .context("Invalid entry ID for branching")?;
            (entry.parent_id.clone().filter(|parent| !parent.is_empty()), user_message_text(&entry.raw["message"]))
        };
        self.abort().await;
        let (config, skills) = self.session_config(true).await?;
        let journal = self.session.journal.lock().await.fork_at(parent.as_deref(), self.sessions.dir.as_deref())?;
        let header = journal.header().clone();
        let messages = journal.model_context();
        self.adopt(Some(journal), header, messages, config, skills).await?;
        Ok(text)
    }

    fn start(&mut self, message: Option<AgentInput>, command: Command) -> Result<()> {
        if self.session.persistence_error.lock().unwrap().is_some() {
            bail!("session persistence failed; restart from the journal before continuing");
        }
        if self.active.is_some() {
            bail!("Agent is already running; specify streamingBehavior: steer or followUp");
        }
        if message.is_some() {
            self.prompt_generation = self.prompt_generation.wrapping_add(1);
        }
        let cancel = self.connection.child_token();
        let mut config = self.config.clone();
        config.deadline = self.max_time.map(|seconds| Instant::now() + Duration::from_secs_f64(seconds.max(0.0)));
        let sink = Arc::new(RunSink {
            session: self.session.clone(),
            output: self.output.clone(),
            cancel: cancel.clone(),
            connection: self.connection.clone(),
            terminal: Mutex::new(None),
            messages: Mutex::new(Vec::new()),
            entries: Mutex::new(Vec::new()),
        });
        let agent = self.agent.clone();
        let run_cancel = cancel.clone();
        let run_sink = sink.clone();
        // The serial Host owns the slot before it can accept another command.
        // ACK was enqueued by the caller before this task can emit AgentStart.
        let task = tokio::spawn(async move {
            // Abort/new/switch may cancel the accepted dispatch before the
            // spawned task enters Agent. Do not append an uninvoked prompt.
            if run_cancel.is_cancelled() {
                return Ok(None);
            }
            match message {
                Some(message) => agent.prompt_inputs_with_config(vec![message], config, run_cancel, run_sink).await,
                None => agent.continue_run_with_config(config, run_cancel, run_sink).await,
            }
            .map(Some)
        });
        self.active = Some(ActiveRun { cancel, task, command, sink });
        Ok(())
    }

    async fn completed(
        &mut self,
        active: ActiveRun,
        result: std::result::Result<std::result::Result<Option<RunReport>, AgentError>, tokio::task::JoinError>,
    ) {
        // Both callers joined the task and removed the active slot. Publish
        // idle only after the Agent's running guard and transcript lock release.
        if let Some(terminal) = active.sink.terminal.lock().unwrap().take() {
            self.output.frame(terminal);
        }
        let persistence_error = active.sink.session.persistence_error.lock().unwrap().clone();
        if persistence_error.is_none()
            && let Ok(Ok(Some(report))) = &result
        {
            let assistant = report.messages.iter().rev().find_map(Message::as_assistant);
            if matches!(report.end, RunEnd::Error | RunEnd::Aborted)
                && let Some(message) = assistant
            {
                match self.begin_retry(&active, message).await {
                    Ok(true) => return,
                    Ok(false) => {}
                    Err(error) => {
                        self.finish_retry(None, Some(format!("Retry continuation failed locally: {error}")), false)
                            .await;
                    }
                }
            } else if report.end == RunEnd::Completed {
                self.finish_retry(assistant, None, false).await;
            }
        }
        let error = match result {
            Ok(Ok(None)) => {
                self.output.prompt_skipped(&active.command);
                None
            }
            Ok(Ok(Some(report))) => match report.end {
                RunEnd::Completed => {
                    self.auto_compaction_pending = true;
                    None
                }
                RunEnd::Aborted => Some("Request was aborted".into()),
                RunEnd::Deadline => Some("Deadline exceeded".into()),
                RunEnd::ModelCallBudget => Some("Model call limit reached".into()),
                RunEnd::Error => report
                    .messages
                    .iter()
                    .rev()
                    .find_map(Message::as_assistant)
                    .and_then(|message| message.error_message.clone())
                    .or_else(|| Some("Agent run failed".into())),
            },
            Ok(Err(error)) => Some(error.to_string()),
            Err(error) => Some(format!("Run failed; tool effects may be unknown: {error}")),
        };
        let error = persistence_error
            .map(|error| format!("Session persistence failed ({error}); restart from the journal"))
            .or(error);
        if let Some(error) = error {
            self.finish_retry(None, Some(error.clone()), false).await;
            self.auto_compaction_pending = false;
            self.drain_queues = false;
            self.output.response(&active.command, None, Some(error));
        }
    }

    async fn abort(&mut self) {
        self.finish_retry(None, Some("Retry cancelled".into()), false).await;
        self.auto_compaction_pending = false;
        self.drain_queues = false;
        if let Some(mut active) = self.active.take() {
            // Covers the interval before Agent::enter installs its own token.
            active.cancel.cancel();
            self.agent.abort();
            let result = (&mut active.task).await;
            self.completed(active, result).await;
        }
        self.flush_pending_bash().await;
        // An already-settling Run can complete successfully after cancellation.
        // An explicit abort must still suppress new automatic maintenance.
        self.auto_compaction_pending = false;
    }

    async fn reconcile_queues(&mut self) {
        if self.active.is_none()
            && self.retry.is_none()
            && self.drain_queues
            && self.agent.has_queued_messages()
            && !self.connection.is_cancelled()
        {
            let mut pending = self.agent.peek_steering_inputs();
            if self.agent.steering_mode() == QueueMode::OneAtATime {
                pending.truncate(1);
            }
            let pending = pending.into_iter().map(|input| input.model).collect::<Vec<_>>();
            self.maybe_auto_compact(&pending).await;
            let command = Command::new(wire(json!({"type":"prompt"})));
            if let Err(error) = self.start(None, command) {
                self.drain_queues = false;
                self.output
                    .frame(json!({"type":"response","command":"prompt","success":false,"error":error.to_string()}));
            }
        }
    }

    fn state(&self) -> Value {
        let (steering, follow_up) = self.agent.queued_counts();
        let model = &self.config.model;
        let mut state = json!({
            "model":{"id":model.id,"provider":model.provider,"api":model.api,"baseUrl":model.base_url,"reasoning":model.reasoning},
            "isStreaming":self.active.is_some(),"isCompacting":self.is_compacting,
            "steeringMode":mode_name(self.agent.steering_mode()),"followUpMode":mode_name(self.agent.follow_up_mode()),
            "interruptMode":"wait", "sessionId":self.session.header["id"],
            "autoCompactionEnabled":self.compaction_policy.enabled(),"fastModeEnabled":false,"fastModeActive":false,
            "autoRetryEnabled":self.retry_policy.enabled(),"isRetrying":self.retry.as_ref().is_some_and(|retry| retry.visible),
            "tokensPerSecond":null,"messageCount":self.session.messages.lock().unwrap().len(),
            "queuedMessageCount":steering+follow_up,"todoPhases":[],"systemPrompt":self.config.system_prompt,
            "dumpTools":self.config.tools.iter().map(|tool| tool.definition()).collect::<Vec<_>>()
        });
        if let Some(file) = &self.session.file {
            state["sessionFile"] = json!(file);
        }
        if let Some(name) = &*self.session.name.lock().unwrap() {
            state["sessionName"] = json!(name);
        }
        state
    }

    async fn handle(&mut self, command: Command) {
        let result = self.execute(&command).await;
        if let Err(error) = result {
            self.output.response(&command, None, Some(error.to_string()));
        }
        self.reconcile_queues().await;
    }

    async fn execute(&mut self, command: &Command) -> Result<()> {
        match command.kind.as_str() {
            "set_auto_retry" => {
                let enabled = match command.frame.get("enabled") {
                    Some(WireValue::Bool(enabled)) => *enabled,
                    _ => bail!("set_auto_retry requires boolean enabled"),
                };
                self.retry_policy.set_enabled(enabled)?;
                self.output.response(command, None, None);
            }
            "abort_retry" => {
                self.abort_retry().await;
                self.output.response(command, None, None);
            }
            "compact" => {
                let focus = command
                    .frame
                    .get("customInstructions")
                    .map(|_| command.string("customInstructions"))
                    .transpose()?;
                let result = self.compact(focus.as_deref()).await?;
                self.output.response(command, Some(result), None);
            }
            "set_auto_compaction" => {
                let enabled = match command.frame.get("enabled") {
                    Some(WireValue::Bool(enabled)) => *enabled,
                    _ => bail!("set_auto_compaction requires boolean enabled"),
                };
                self.compaction_policy.set_enabled(enabled)?;
                self.auto_compaction_checked = None;
                self.output.response(command, None, None);
            }
            "bash" => self.bash_dispatcher.dispatch(Command {
                id: command.id.clone(),
                kind: command.kind.clone(),
                frame: command.frame.clone(),
            }),
            "abort_bash" => {
                self.bash_dispatcher.abort();
                self.output.response(command, None, None);
            }
            "set_host_tools" => {
                let definitions = normalize_host_tool_definitions(
                    command.frame.get("tools").context("set_host_tools requires tools")?,
                )
                .map_err(anyhow::Error::msg)?;
                let names = self.set_host_tools(definitions).await?;
                self.output.response(command, Some(json!({"toolNames":names})), None);
            }
            "set_host_uri_schemes" => {
                let schemes = self
                    .uri_bridge
                    .set_schemes(command.frame.get("schemes").context("set_host_uri_schemes requires schemes")?)
                    .map_err(anyhow::Error::msg)?;
                self.output.response(command, Some(json!({"schemes":schemes})), None);
            }
            "negotiate_protocol" => {
                if command.frame.get("protocolVersion").and_then(WireValue::as_number) != Some(2.0) {
                    bail!("Unsupported RPC protocol version; expected 2");
                }
                self.output.response(command, Some(json!({"protocolVersion":2})), None);
            }
            "get_state" => self.output.response(command, Some(self.state()), None),
            "get_messages" => {
                self.output.response(command, Some(json!({"messages":*self.session.messages.lock().unwrap()})), None)
            }
            "get_messages_page" => {
                use ara_rpc::messages::{
                    RPC_MESSAGES_PAGE_BUSY_ERROR, RpcMessageSnapshot, RpcMessagesPageOptions, page_rpc_messages,
                };
                if self.active.is_some() {
                    self.output.response_wire(
                        command,
                        None,
                        Some(RPC_MESSAGES_PAGE_BUSY_ERROR.into()),
                        Some("session_busy"),
                    );
                } else {
                    let journal = self.session.journal.lock().await;
                    let messages = self.session.messages.lock().unwrap().iter().cloned().map(wire).collect::<Vec<_>>();
                    let snapshot = RpcMessageSnapshot {
                        session_id: self.session.header["id"].as_str().context("Session identity is missing")?.into(),
                        leaf_id: journal.leaf_id().map(Into::into),
                        message_count: messages.len(),
                    };
                    match page_rpc_messages(
                        &messages,
                        &snapshot,
                        RpcMessagesPageOptions {
                            cursor: command.frame.get("cursor"),
                            limit: command.frame.get("limit"),
                        },
                    ) {
                        Ok(page) => self.output.response_wire(command, Some(page.into()), None, None),
                        Err(error) => self.output.response_wire(command, None, Some(error.message), error.code),
                    }
                }
            }
            "get_session_stats" => {
                let journal = self.session.journal.lock().await;
                let messages = if journal.branch().iter().any(|entry| entry.kind == "compaction") {
                    let projection =
                        journal.compacted_context_projection().context("projecting compacted Session statistics")?;
                    let entries = journal
                        .entries()
                        .iter()
                        .map(|entry| (entry.id.as_str(), entry))
                        .collect::<std::collections::HashMap<_, _>>();
                    projection
                        .items
                        .into_iter()
                        .map(|item| match item {
                            ara_session::CompactedContextItem::Summary(_) => json!({"role":"compactionSummary"}),
                            ara_session::CompactedContextItem::Message(sourced) => entries
                                .get(sourced.entry_id.as_str())
                                .and_then(|entry| {
                                    entry
                                        .bash_execution()
                                        .map(|bash| bash.event_message())
                                        .or_else(|| entry.skill_prompt().map(|prompt| prompt.event_message()))
                                })
                                .unwrap_or_else(|| {
                                    AgentEvent::MessageEnd { message: sourced.message }.full()["message"].clone()
                                }),
                        })
                        .collect::<Vec<_>>()
                } else {
                    self.session.messages.lock().unwrap().clone()
                };
                let mut stats = session_stats(&messages);
                stats["sessionId"] = self.session.header["id"].clone();
                if let Some(file) = &self.session.file {
                    stats["sessionFile"] = json!(file);
                }
                self.output.response(command, Some(stats), None);
            }
            "get_branch_messages" => {
                let journal = self.session.journal.lock().await;
                let messages =
                    journal.entries().iter().filter_map(|entry| branch_user_message(&entry.raw)).collect::<Vec<_>>();
                self.output.response(command, Some(json!({"messages":messages})), None);
            }
            "set_session_name" => {
                let name = command.string("name")?;
                let name = ara_prompt::js::trim(&name);
                if name.is_empty() {
                    bail!("Session name cannot be empty");
                }
                let mut journal = self.session.journal.lock().await;
                if !journal.set_session_name(name, "user")? {
                    bail!("Session name cannot be empty");
                }
                let applied = journal.title().title.clone();
                *self.session.name.lock().unwrap() = Some(applied);
                self.output.response(command, None, None);
            }
            "get_last_assistant_text" => {
                let text = last_assistant_text(&self.session.messages.lock().unwrap());
                let data = text.map_or_else(|| json!({}), |text| json!({"text":text}));
                self.output.response(command, Some(data), None);
            }
            "get_available_commands" => {
                self.output.response(command, Some(json!({"commands":self.available_commands()})), None)
            }
            "set_steering_mode" | "set_follow_up_mode" => {
                let mode = command.queue_mode()?;
                if command.kind == "set_steering_mode" {
                    self.agent.set_steering_mode(mode);
                } else {
                    self.agent.set_follow_up_mode(mode);
                }
                self.output.response(command, None, None);
            }
            "steer" | "follow_up" => {
                let message = command.message()?;
                if command.kind == "steer" {
                    self.agent.steer(message);
                } else {
                    self.agent.follow_up(message);
                }
                self.drain_queues = true;
                self.output.response(command, None, None);
            }
            "abort" => {
                self.abort().await;
                self.output.response(command, None, None);
            }
            "new_session" => {
                let parent = command.frame.get("parentSession").map(|_| command.string("parentSession")).transpose()?;
                self.new_session(parent.as_deref()).await?;
                self.emit_available_commands();
                self.output.response(command, Some(json!({"cancelled":false})), None);
            }
            "switch_session" => {
                let path = PathBuf::from(command.string("sessionPath")?);
                let cancelled = self.switch_session(path).await?;
                if !cancelled {
                    self.emit_available_commands();
                }
                self.output.response(command, Some(json!({"cancelled":cancelled})), None);
            }
            "branch" => {
                let text = self.branch(&command.string("entryId")?).await?;
                self.emit_available_commands();
                self.output.response(command, Some(json!({"text":text,"cancelled":false})), None);
            }
            "prompt" => {
                let text = command.string("message")?;
                let deadline = self.max_time.map(|seconds| Instant::now() + Duration::from_secs_f64(seconds.max(0.0)));
                let prepared = super::skill_command::prepare(&text, &text, &self.skills, &self.connection, deadline)
                    .await
                    .map_err(|error| match error {
                        super::skill_command::PreparationError::Load(error) => {
                            anyhow::anyhow!("Failed to load Skill: {error}")
                        }
                        super::skill_command::PreparationError::Cancelled => anyhow::anyhow!("Request was aborted"),
                        super::skill_command::PreparationError::Deadline => anyhow::anyhow!("Deadline exceeded"),
                    })?;
                let recognized = prepared.is_some();
                // Resolve the registered Skill before ordinary image parsing.
                // Fixed RPC ignores images entirely for this entrypoint.
                let input = match prepared {
                    Some(prompt) => skill_input(prompt),
                    None => command.message()?.into(),
                };
                let queue = if let Some(value) = command.frame.get("streamingBehavior") {
                    match value.as_string().and_then(|s| s.to_utf8().ok()).as_deref() {
                        Some("steer") => Some(true),
                        Some("followUp") => Some(false),
                        _ => bail!("streamingBehavior must be steer or followUp"),
                    }
                } else {
                    recognized.then_some(true)
                };
                self.output.response(command, recognized.then(|| json!({"agentInvoked":true})), None);
                self.drain_queues = true;
                if self.active.is_some()
                    && let Some(steer) = queue
                {
                    if steer {
                        self.agent.steer_input(input);
                    } else {
                        self.agent.follow_up_input(input);
                    }
                } else {
                    if self.active.is_none() {
                        self.finish_retry(None, Some("Retry cancelled".into()), false).await;
                    }
                    self.maybe_auto_compact(std::slice::from_ref(&input.model)).await;
                    self.start(
                        Some(input),
                        Command { id: command.id.clone(), kind: command.kind.clone(), frame: command.frame.clone() },
                    )?;
                }
            }
            "abort_and_prompt" => {
                let message = command.message()?;
                self.abort().await;
                self.maybe_auto_compact(std::slice::from_ref(&message)).await;
                self.output.response(command, None, None);
                self.drain_queues = true;
                self.start(
                    Some(message.into()),
                    Command { id: command.id.clone(), kind: command.kind.clone(), frame: command.frame.clone() },
                )?;
            }
            "extension_ui_response" | "host_tool_result" | "host_tool_update" | "host_uri_result" => {
                bail!("RPC side channel is not implemented yet");
            }
            "" => bail!("RPC command requires a string type"),
            _ => bail!("RPC command {} is not implemented yet", command.kind),
        }
        Ok(())
    }
}

fn mode_name(mode: QueueMode) -> &'static str {
    match mode {
        QueueMode::All => "all",
        QueueMode::OneAtATime => "one-at-a-time",
    }
}

fn branch_user_message(entry: &Value) -> Option<Value> {
    if entry["type"] != "message" || entry["message"]["role"] != "user" {
        return None;
    }
    let text = user_message_text(&entry["message"]);
    (!text.is_empty()).then(|| json!({"entryId":entry["id"],"text":text}))
}

fn user_message_text(message: &Value) -> String {
    let content = &message["content"];
    content.as_str().map(str::to_owned).unwrap_or_else(|| {
        content
            .as_array()
            .into_iter()
            .flatten()
            .filter(|block| block["type"] == "text")
            .filter_map(|block| block["text"].as_str())
            .collect::<String>()
    })
}

fn session_stats(messages: &[Value]) -> Value {
    let count = |role: &str| messages.iter().filter(|message| message["role"] == role).count();
    let assistants = messages.iter().filter(|message| message["role"] == "assistant").collect::<Vec<_>>();
    // Any missing provider bucket makes that aggregate unknown. A successful
    // response without pricing metadata is not a zero-cost response.
    let sum = |field: &str| {
        assistants.iter().try_fold(0u64, |sum, message| sum.checked_add(message["usage"][field].as_u64()?))
    };
    let cost = assistants.iter().try_fold(0.0, |sum, message| Some(sum + message["usage"]["cost"]["total"].as_f64()?));
    let tool_calls = assistants
        .iter()
        .map(|message| {
            message["content"].as_array().into_iter().flatten().filter(|block| block["type"] == "toolCall").count()
        })
        .sum::<usize>();
    json!({"userMessages":count("user"),"assistantMessages":assistants.len(),
        "toolCalls":tool_calls,"toolResults":count("toolResult"),"totalMessages":messages.len(),
        "tokens":{"input":sum("input"),"output":sum("output"),"reasoning":sum("reasoningTokens"),
            "cacheRead":sum("cacheRead"),"cacheWrite":sum("cacheWrite"),"total":sum("totalTokens")},
        "cost":cost,"premiumRequests":if assistants.is_empty() {json!(0)} else {Value::Null}})
}

// Fixed session/messages.ts:194-246 compares provider replay values rather
// than timestamps, usage or other runtime metadata. Only the four native Rust
// message roles are supported here; custom Session messages remain open.
fn replay_value(message: &Message) -> Value {
    match message {
        Message::User(message) => json!({"role":"user","content":message.content}),
        Message::Developer(message) => json!({"role":"developer","content":message.content}),
        Message::ToolResult(message) => json!({
            "role":"toolResult","toolName":message.tool_name,"toolCallId":message.tool_call_id,
            "isError":message.is_error,"content":message.content
        }),
        Message::Assistant(message) => {
            let responses = matches!(message.api.as_str(), "openai-responses" | "openai-codex-responses");
            let content = if responses {
                message
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ara_ai::AssistantBlock::Thinking(_) => None,
                        ara_ai::AssistantBlock::ToolCall(call) => Some(json!({
                            "type":"toolCall","id":call.id,"name":call.name,"arguments":call.arguments
                        })),
                        ara_ai::AssistantBlock::Text(text) => Some(json!({
                            "type":"text","text":text.text,"textSignature":text.text_signature
                        })),
                        block => Some(serde_json::to_value(block).expect("message block")),
                    })
                    .collect::<Vec<_>>()
            } else {
                message.content.iter().map(|block| serde_json::to_value(block).expect("message block")).collect()
            };
            json!({
                "role":"assistant","content":content,"api":message.api,"provider":message.provider,
                "model":message.model,"stopReason":message.stop_reason,"errorMessage":message.error_message,
                "providerPayload":if responses { None } else { message.provider_payload.as_ref() }
            })
        }
    }
}

fn replay_messages_equal(left: &[Message], right: &[Message]) -> bool {
    left.len() == right.len() && left.iter().zip(right).all(|(a, b)| replay_value(a) == replay_value(b))
}

fn last_assistant_text(messages: &[Value]) -> Option<String> {
    let assistant = messages.iter().rev().find(|message| {
        message["role"] == "assistant"
            && !(message["stopReason"] == "aborted" && message["content"].as_array().is_some_and(Vec::is_empty))
    })?;
    let text = assistant["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|block| block["type"] == "text")
        .filter_map(|block| block["text"].as_str())
        .collect::<String>();
    let text = ara_prompt::js::trim(&text);
    (!text.is_empty()).then(|| text.to_owned())
}

pub async fn run(
    config: AgentConfig,
    skills: Vec<LoadedSkill>,
    messages: Vec<Message>,
    journal: Option<SessionJournal>,
    header: Value,
    max_time: Option<f64>,
    sessions: SessionFactory,
) -> Result<i32> {
    serve(tokio::io::stdin(), tokio::io::stdout(), config, skills, messages, journal, header, max_time, sessions).await
}

#[allow(clippy::too_many_arguments)]
async fn serve<R, W>(
    input: R,
    writer: W,
    mut config: AgentConfig,
    skills: Vec<LoadedSkill>,
    messages: Vec<Message>,
    journal: Option<SessionJournal>,
    header: Value,
    max_time: Option<f64>,
    mut sessions: SessionFactory,
) -> Result<i32>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let compaction_policy = AutoCompactionPolicy::load(&super::ara_home().join("agent"))?;
    let retry_policy = RetryPolicy::load(&super::ara_home().join("agent"))?;
    let connection = CancellationToken::new();
    let (output_tx, output_rx) = mpsc::unbounded_channel();
    let output = Output(output_tx);
    let output_task = tokio::spawn(write_output(writer, output_rx, connection.clone()));
    let emitter_output = output.clone();
    let emitter: Arc<dyn Fn(WireValue) + Send + Sync> =
        Arc::new(move |frame| emitter_output.send(OutputItem::Frame(frame)));
    let tool_bridge = ToolBridge::new(emitter.clone());
    let uri_bridge = UriBridge::new(emitter);
    if let Some(context) = sessions.tool_context.take() {
        context.set_uri_port(uri_bridge.clone());
    }
    sessions.uri_port = Some(uri_bridge.clone());
    let snapshot = Arc::new(RwLock::new(ExecutionSnapshot {
        tools: config.tools.clone(),
        system_prompt: config.system_prompt.clone(),
    }));
    config.hooks = Arc::new(RpcHooks { base: config.hooks.clone(), snapshot: snapshot.clone() });
    let session = Session::new(journal, header, &messages)?;
    let bash_target =
        Arc::new(Mutex::new(BashTarget { session: session.clone(), destination: BashDestination::Current }));
    let (bash_done_tx, mut bash_done_rx) = mpsc::unbounded_channel();
    let bash_dispatcher = Arc::new(BashDispatcher {
        current: Mutex::new(bash_target.clone()),
        jobs: Mutex::new(std::collections::HashMap::new()),
        next_id: std::sync::atomic::AtomicU64::new(1),
        done: bash_done_tx,
        output: output.clone(),
        connection: connection.clone(),
        cwd: sessions.cwd.clone(),
    });
    let mut host = Host {
        agent: Agent::new(config.clone(), messages),
        session,
        config,
        skills,
        max_time,
        output: output.clone(),
        connection: connection.clone(),
        active: None,
        sessions,
        tool_bridge: tool_bridge.clone(),
        uri_bridge: uri_bridge.clone(),
        host_tools: Vec::new(),
        snapshot,
        drain_queues: true,
        bash_targets: vec![Arc::downgrade(&bash_target)],
        bash_target,
        bash_dispatcher: bash_dispatcher.clone(),
        pending_bash: Vec::new(),
        bash_error: None,
        compaction_policy,
        is_compacting: false,
        auto_compaction_pending: false,
        auto_compaction_checked: None,
        retry_policy,
        retry: None,
        prompt_generation: 0,
    };
    output.frame(json!({"type":"ready","protocolVersion":1,"supportedProtocolVersions":[1,2],
        "maxFrameBytes":MAX_RPC_FRAME_BYTES,"maxReassembledFrameBytes":MAX_RPC_REASSEMBLED_BYTES}));
    host.emit_available_commands();
    let (input_tx, mut input_rx) = mpsc::unbounded_channel();
    let reader_output = output.clone();
    let reader_cancel = connection.clone();
    let reader_task = tokio::spawn(async move {
        let mut reader = RpcInputReader::new(input);
        loop {
            let next = tokio::select! {
                biased;
                _ = reader_cancel.cancelled() => break,
                item = reader.read_next() => item,
            };
            match next {
                Ok(Some(InputItem::Frame(frame))) => {
                    if tool_bridge.consume(&frame) || uri_bridge.consume(&frame) {
                        continue;
                    }
                    let command = Command::new(frame);
                    if command.kind == "bash" {
                        bash_dispatcher.dispatch(command);
                        continue;
                    }
                    if input_tx.send(command).is_err() {
                        break;
                    }
                }
                Ok(Some(InputItem::ParseError(error))) => {
                    reader_output.frame(json!({"type":"response","command":"parse","success":false,"error":error}))
                }
                Ok(None) => break,
                Err(error) => {
                    tool_bridge.close("RPC input disconnected");
                    uri_bridge.close_connection("RPC input disconnected");
                    reader_cancel.cancel();
                    return Err(error);
                }
            }
        }
        // Unblock accepted work before the serial command owner drains/joins.
        tool_bridge.close("RPC host disconnected");
        uri_bridge.close_connection("RPC host disconnected");
        Ok(())
    });
    let mut eof = false;
    loop {
        host.flush_pending_bash().await;
        if std::mem::take(&mut host.auto_compaction_pending) && !eof {
            host.maybe_auto_compact(&[]).await;
        }
        host.reconcile_queues().await;
        if eof && host.active.is_none() && host.retry.is_none() && host.bash_dispatcher.is_empty() {
            break;
        }
        tokio::select! {
            biased;
            _ = connection.cancelled() => {
                host.abort().await;
                host.bash_dispatcher.abort();
                while !host.bash_dispatcher.is_empty() {
                    if let Some(completion) = bash_done_rx.recv().await {
                        host.completed_bash(completion).await;
                    }
                }
                host.flush_pending_bash().await;
                break;
            }
            completion = bash_done_rx.recv() => {
                if let Some(completion) = completion {
                    host.completed_bash(completion).await;
                }
            }
            result = async {
                match &mut host.active {
                    Some(active) => (&mut active.task).await,
                    None => pending().await,
                }
            }, if host.active.is_some() => {
                let active = host.active.take().expect("selected active Run");
                host.completed(active, result).await;
            }
            _ = async {
                match host.retry.as_ref().and_then(|retry| retry.deadline) {
                    Some(deadline) => tokio::time::sleep_until(deadline.into()).await,
                    None => pending().await,
                }
            }, if host.retry.as_ref().is_some_and(|retry| retry.deadline.is_some()) => {
                host.resume_retry().await;
            }
            command = input_rx.recv(), if !eof => {
                match command {
                    Some(command) => host.handle(command).await,
                    None => eof = true,
                }
            }
        }
    }
    let input_result = reader_task.await.context("RPC input task");
    output.frame(json!({"type":"session_shutdown"}));
    let failed = host.bash_error.clone().or_else(|| host.session.persistence_error.lock().unwrap().clone());
    drop(host);
    drop(output);
    output_task.await.context("RPC output task")??;
    input_result?.context("reading RPC input")?;
    if let Some(error) = failed {
        eprintln!("ara: session persistence failed ({error}); restart from the journal");
        Ok(1)
    } else {
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use tokio::sync::oneshot;

    struct TerminalGate {
        inner: Arc<RunSink>,
        reached: Mutex<Option<oneshot::Sender<()>>>,
        release: tokio::sync::Mutex<Option<oneshot::Receiver<()>>>,
    }

    #[async_trait]
    impl AgentEventSink for TerminalGate {
        async fn emit_input(&self, input: AgentInput) {
            self.inner.emit_input(input).await;
        }

        async fn emit(&self, event: AgentEvent) {
            let terminal = matches!(event, AgentEvent::AgentEnd { .. });
            self.inner.emit(event).await;
            if terminal {
                self.reached.lock().unwrap().take().unwrap().send(()).unwrap();
                self.release.lock().await.take().unwrap().await.unwrap();
            }
        }
    }

    fn frames(rx: &mut mpsc::UnboundedReceiver<OutputItem>) -> Vec<Value> {
        std::iter::from_fn(|| rx.try_recv().ok())
            .map(|item| match item {
                OutputItem::Frame(frame) | OutputItem::Negotiate(frame) => {
                    serde_json::from_str(&frame.stringify()).unwrap()
                }
            })
            .collect()
    }

    fn fixture() -> (Host, mpsc::UnboundedReceiver<OutputItem>) {
        let cwd = std::env::current_dir().unwrap();
        let provider = super::super::ProviderFactory {
            client: reqwest::Client::new(),
            api: super::super::Api::OpenaiCompletions,
            options: Default::default(),
            anthropic_strict_tools: false,
            responses_stateful: false,
        };
        let config = AgentConfig {
            model: ara_ai::Model {
                id: "terminal-fixture".into(),
                api: "openai-completions".into(),
                provider: "test".into(),
                base_url: String::new(),
                reasoning: false,
                max_tokens: None,
                tokenizer: None,
            },
            provider: provider.build(),
            system_prompt: Vec::new(),
            tools: Vec::new(),
            tool_choice: None,
            max_tokens: None,
            temperature: None,
            deadline: None,
            // Exercise real Agent ownership without a network/model call.
            max_model_calls: Some(0),
            hooks: Arc::new(ara_agent::NoHooks),
        };
        let (tx, rx) = mpsc::unbounded_channel();
        let output = Output(tx);
        let connection = CancellationToken::new();
        let session = Session::new(None, super::super::ephemeral_header(&cwd, None), &[]).unwrap();
        let bash_target =
            Arc::new(Mutex::new(BashTarget { session: session.clone(), destination: BashDestination::Current }));
        let (bash_done_tx, _) = mpsc::unbounded_channel();
        let bash_dispatcher = Arc::new(BashDispatcher {
            current: Mutex::new(bash_target.clone()),
            jobs: Mutex::new(std::collections::HashMap::new()),
            next_id: std::sync::atomic::AtomicU64::new(1),
            done: bash_done_tx,
            output: output.clone(),
            connection: connection.clone(),
            cwd: cwd.clone(),
        });
        let emitter_output = output.clone();
        let emitter: Arc<dyn Fn(WireValue) + Send + Sync> =
            Arc::new(move |frame| emitter_output.send(OutputItem::Frame(frame)));
        let snapshot = Arc::new(RwLock::new(ExecutionSnapshot {
            tools: config.tools.clone(),
            system_prompt: config.system_prompt.clone(),
        }));
        let host = Host {
            agent: Agent::new(config.clone(), Vec::new()),
            session: session.clone(),
            config: config.clone(),
            skills: Vec::new(),
            max_time: None,
            output: output.clone(),
            connection: connection.clone(),
            active: None,
            sessions: SessionFactory {
                dir: None,
                cwd,
                provider,
                args: super::super::Args::parse_from(["ara"]),
                mcp_config: None,
                tool_context: None,
                uri_port: None,
            },
            tool_bridge: ToolBridge::new(emitter.clone()),
            uri_bridge: UriBridge::new(emitter),
            host_tools: Vec::new(),
            snapshot,
            drain_queues: false,
            bash_targets: vec![Arc::downgrade(&bash_target)],
            bash_target,
            bash_dispatcher,
            pending_bash: Vec::new(),
            bash_error: None,
            compaction_policy: AutoCompactionPolicy::isolated(true),
            is_compacting: false,
            auto_compaction_pending: false,
            auto_compaction_checked: None,
            retry_policy: RetryPolicy::isolated(true),
            retry: None,
            prompt_generation: 0,
        };
        (host, rx)
    }

    #[tokio::test]
    async fn bash_dispatch_starts_child_while_serial_abort_waits_for_run_settlement() {
        let dir = tempfile::tempdir().unwrap();
        let (mut host, mut frames_rx) = fixture();
        let (done_tx, mut done_rx) = mpsc::unbounded_channel();
        let dispatcher = Arc::new(BashDispatcher {
            current: Mutex::new(host.bash_target.clone()),
            jobs: Mutex::new(std::collections::HashMap::new()),
            next_id: std::sync::atomic::AtomicU64::new(1),
            done: done_tx,
            output: host.output.clone(),
            connection: host.connection.clone(),
            cwd: dir.path().into(),
        });
        host.bash_dispatcher = dispatcher.clone();
        let cancel = CancellationToken::new();
        let sink = Arc::new(RunSink {
            session: host.session.clone(),
            output: host.output.clone(),
            cancel: cancel.clone(),
            connection: host.connection.clone(),
            terminal: Mutex::new(None),
            messages: Mutex::new(Vec::new()),
            entries: Mutex::new(Vec::new()),
        });
        let (reached_tx, reached_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let gate = Arc::new(TerminalGate {
            inner: sink.clone(),
            reached: Mutex::new(Some(reached_tx)),
            release: tokio::sync::Mutex::new(Some(release_rx)),
        });
        let agent = host.agent.clone();
        let config = host.config.clone();
        let run_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            agent
                .prompt_inputs_with_config(
                    vec![
                        Message::User(UserMessage {
                            content: UserContent::Text("gated".into()),
                            synthetic: None,
                            timestamp: 0,
                        })
                        .into(),
                    ],
                    config,
                    run_cancel,
                    gate,
                )
                .await
                .map(Some)
        });
        host.active = Some(ActiveRun { cancel, task, command: Command::new(wire(json!({"type":"prompt"}))), sink });
        tokio::time::timeout(Duration::from_secs(3), reached_rx).await.unwrap().unwrap();
        let serial = tokio::spawn(async move {
            host.abort().await;
            host
        });
        dispatcher.dispatch(Command::new(wire(
            json!({"type":"bash","id":"overtake", "command":"printf admitted > admitted.txt"}),
        )));
        let completion = tokio::time::timeout(Duration::from_secs(4), done_rx.recv()).await.unwrap().unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("admitted.txt")).unwrap(), "admitted");
        assert!(!serial.is_finished(), "Bash must run before serial Run settlement is released");
        release_tx.send(()).unwrap();
        let mut host = tokio::time::timeout(Duration::from_secs(3), serial).await.unwrap().unwrap();
        host.completed_bash(completion).await;
        let output = frames(&mut frames_rx);
        assert!(output.iter().any(|frame| frame["id"] == "overtake" && frame["success"] == true));
        assert_eq!(
            host.session.messages.lock().unwrap().iter().filter(|message| message["role"] == "bashExecution").count(),
            1
        );
        assert!(host.bash_dispatcher.is_empty());
    }

    #[tokio::test]
    async fn bash_receipt_write_failure_preserves_process_effect_and_blocks_next_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let (mut host, mut output_rx) = fixture();
        let mut journal = SessionJournal::create(dir.path(), dir.path()).unwrap();
        journal.materialize().unwrap();
        let path = journal.path().to_path_buf();
        let header = journal.header().clone();
        host.session = Session::new(Some(journal), header, &[]).unwrap();
        let target =
            Arc::new(Mutex::new(BashTarget { session: host.session.clone(), destination: BashDestination::Current }));
        let (done_tx, mut done_rx) = mpsc::unbounded_channel();
        host.bash_target = target.clone();
        host.bash_targets = vec![Arc::downgrade(&target)];
        host.bash_dispatcher = Arc::new(BashDispatcher {
            current: Mutex::new(target),
            jobs: Mutex::new(std::collections::HashMap::new()),
            next_id: std::sync::atomic::AtomicU64::new(1),
            done: done_tx,
            output: host.output.clone(),
            connection: host.connection.clone(),
            cwd: dir.path().into(),
        });
        host.bash_dispatcher.dispatch(Command::new(wire(
            json!({"type":"bash","id":"disk-fault", "command":"printf happened > effect.txt"}),
        )));
        let completion = tokio::time::timeout(Duration::from_secs(4), done_rx.recv()).await.unwrap().unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("effect.txt")).unwrap(), "happened");
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        host.completed_bash(completion).await;
        assert!(host.connection.is_cancelled());
        assert!(host.session.persistence_error.lock().unwrap().is_some());
        assert!(host.bash_error.as_deref().unwrap().contains("effects may have occurred"));
        assert!(host.start(None, Command::new(wire(json!({"type":"prompt"})))).is_err());
        assert!(!host.agent.is_busy());
        assert!(frames(&mut output_rx).iter().any(|frame| frame["id"] == "disk-fault" && frame["success"] == false));
        assert_eq!(std::fs::read_to_string(dir.path().join("effect.txt")).unwrap(), "happened");
    }

    #[test]
    fn metadata_keeps_hidden_skills_and_falls_back_without_source_fields() {
        let (mut host, _) = fixture();
        host.skills.push(LoadedSkill {
            name: "隐藏🦀".into(),
            description: String::new(),
            file_path: PathBuf::from("private/SKILL.md"),
            base_dir: PathBuf::from("private"),
            source: "fixture:project".into(),
            hide: true,
            meta: ara_discovery::SourceMeta::new(
                "fixture",
                std::path::Path::new("private"),
                ara_discovery::Level::Project,
            ),
        });
        assert_eq!(
            host.available_commands(),
            vec![json!({"name":"skill:隐藏🦀",
            "description":"Run 隐藏🦀 skill","input":{"hint":"arguments"},"source":"skill"})]
        );
    }

    #[test]
    fn paging_error_keeps_unpaired_utf16_correlation_id_and_code() {
        let (host, mut rx) = fixture();
        let command = Command::new(WireValue::parse(r#"{"type":"get_messages_page","id":"\ud800"}"#).unwrap());
        host.output.response_wire(&command, None, Some("RPC message cursor is stale".into()), Some("stale_cursor"));
        let OutputItem::Frame(frame) = rx.try_recv().unwrap() else { panic!("ordinary response") };
        assert_eq!(frame.get("id"), command.id.as_ref());
        assert_eq!(frame.get("code"), Some(&WireValue::String("stale_cursor".into())));
        assert!(frame.get("data").is_none());
    }

    #[test]
    fn stats_preserve_unknown_buckets_and_custom_identity() {
        let messages = vec![
            json!({"role":"user"}),
            json!({"role":"custom","customType":"skill-prompt"}),
            json!({"role":"assistant","content":[{"type":"toolCall"}],
                "usage":{"input":5,"output":2,"cacheRead":0,"totalTokens":7,"cost":{"total":0.5}}}),
            json!({"role":"toolResult"}),
            json!({"role":"assistant","content":[],"usage":{"input":3,"totalTokens":3}}),
        ];
        let stats = session_stats(&messages);
        assert_eq!(stats["userMessages"], 1);
        assert_eq!(stats["assistantMessages"], 2);
        assert_eq!(stats["toolCalls"], 1);
        assert_eq!(stats["toolResults"], 1);
        assert_eq!(stats["totalMessages"], 5);
        assert_eq!(
            stats["tokens"],
            json!({"input":8,"output":null,"reasoning":null,
            "cacheRead":null,"cacheWrite":null,"total":10})
        );
        assert!(stats["cost"].is_null());
        assert!(stats["premiumRequests"].is_null());
        assert_eq!(session_stats(&[])["cost"], 0.0);
    }

    #[tokio::test]
    async fn cancelled_before_entry_keeps_origin_and_exact_skip_id() {
        let (mut host, mut rx) = fixture();
        // A lone UTF-16 surrogate exercises IDs which serde_json cannot
        // represent. No await before abort: the spawned admission is cancelled.
        let command = Command::new(WireValue::parse(r#"{"type":"prompt","id":"\ud800"}"#).unwrap());
        let id = command.id.clone().unwrap();
        let origin = host.session.clone();
        host.start(Some(skill_input(UserSkillPrompt::new(UserContent::Text("not invoked".into()), None))), command)
            .unwrap();
        host.abort().await;
        assert!(host.agent.messages().await.is_empty());
        assert!(origin.messages.lock().unwrap().is_empty());
        assert!(!host.agent.is_busy());
        let item = rx.try_recv().unwrap();
        let OutputItem::Frame(frame) = item else { panic!("unexpected negotiation") };
        assert_eq!(frame.get("type"), Some(&wire(json!("prompt_result"))));
        assert_eq!(frame.get("agentInvoked"), Some(&WireValue::Bool(false)));
        assert_eq!(frame.get("id"), Some(&id));
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn custom_receipt_failure_cancels_before_provider_and_tool_effects() {
        let dir = tempfile::tempdir().unwrap();
        let mut journal = SessionJournal::create(dir.path(), dir.path()).unwrap();
        journal.materialize().unwrap();
        let file = journal.path().to_path_buf();
        let header = journal.header().clone();
        // The real writer fails its next append without touching other files.
        std::fs::remove_file(&file).unwrap();
        std::fs::create_dir(&file).unwrap();
        let upstream =
            ara_testkit::FakeUpstream::start(ara_testkit::Script { responses: Vec::new() }, None).await.unwrap();
        let (mut host, mut rx) = fixture();
        host.session = Session::new(Some(journal), header, &[]).unwrap();
        let origin = host.session.clone();
        host.config.model.base_url = upstream.base_url();
        host.config.max_model_calls = Some(1);
        host.start(
            Some(skill_input(UserSkillPrompt::new(UserContent::Text("receipt".into()), None))),
            Command::new(wire(json!({"type":"prompt","id":"failed-receipt"}))),
        )
        .unwrap();
        let mut active = host.active.take().unwrap();
        let result = tokio::time::timeout(Duration::from_secs(5), &mut active.task).await.unwrap();
        assert_eq!(result.as_ref().unwrap().as_ref().unwrap().as_ref().unwrap().end, RunEnd::Aborted);
        host.completed(active, result).await;
        assert!(origin.persistence_error.lock().unwrap().is_some());
        assert!(host.connection.is_cancelled());
        assert!(upstream.requests.lock().await.is_empty(), "failed custom receipt must precede model/tool effects");
        let emitted = frames(&mut rx);
        assert_eq!(
            emitted
                .iter()
                .filter(|frame| frame["type"] == "message_end" && frame["message"]["role"] == "custom")
                .count(),
            1
        );
        assert!(emitted.iter().all(|frame| frame["type"] != "tool_execution_start"));
        let failure = emitted.iter().find(|frame| frame["type"] == "response").unwrap();
        assert_eq!(failure["id"], "failed-receipt");
        assert!(failure["error"].as_str().unwrap().contains("Session persistence failed"));
    }

    #[tokio::test]
    async fn terminal_is_published_only_after_owned_run_settles() {
        let (mut host, mut rx) = fixture();
        let connection = host.connection.clone();
        let session = host.session.clone();
        let output = host.output.clone();
        let config = host.config.clone();
        let cancel = connection.child_token();
        let sink = Arc::new(RunSink {
            session,
            output,
            cancel: cancel.clone(),
            connection,
            terminal: Mutex::new(None),
            messages: Mutex::new(Vec::new()),
            entries: Mutex::new(Vec::new()),
        });
        let (reached_tx, reached) = oneshot::channel();
        let (release, release_rx) = oneshot::channel();
        let gate = Arc::new(TerminalGate {
            inner: sink.clone(),
            reached: Mutex::new(Some(reached_tx)),
            release: tokio::sync::Mutex::new(Some(release_rx)),
        });
        let agent = host.agent.clone();
        let run_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            agent
                .prompt_inputs_with_config(
                    vec![skill_input(UserSkillPrompt::new(
                        UserContent::Text("first".into()),
                        Some(json!({"originalText":"/skill:proof"})),
                    ))],
                    config,
                    run_cancel,
                    gate,
                )
                .await
                .map(Some)
        });
        host.active =
            Some(ActiveRun { cancel, task, command: Command::new(wire(json!({"type":"prompt","id":"first"}))), sink });

        tokio::time::timeout(Duration::from_secs(5), reached).await.unwrap().unwrap();
        assert!(!host.active.as_ref().unwrap().task.is_finished());
        assert_eq!(host.state()["isStreaming"], true);
        let live = frames(&mut rx);
        assert!(live.iter().any(|frame| frame["type"] == "agent_start"));
        let input = live.iter().find(|frame| frame["type"] == "message_end").unwrap()["message"].clone();
        assert_eq!(input["customType"], "skill-prompt");
        assert!(!live.iter().any(|frame| frame["type"] == "agent_end"));

        release.send(()).unwrap();
        let mut active = host.active.take().unwrap();
        let result = tokio::time::timeout(Duration::from_secs(5), &mut active.task).await.unwrap();
        host.completed(active, result).await;
        assert_eq!(host.state()["isStreaming"], false);
        let settled = frames(&mut rx);
        assert_eq!(settled.iter().filter(|frame| frame["type"] == "agent_end").count(), 1);
        assert_eq!(settled[0]["type"], "agent_end");
        assert_eq!(settled[0]["messages"][0], input);
        assert_eq!(settled[1]["id"], "first");
        assert_eq!(settled[1]["error"], "Model call limit reached");

        host.handle(Command::new(wire(json!({"type":"prompt","id":"next","message":"next"})))).await;
        let mut active = host.active.take().expect("immediate next prompt starts");
        let result = tokio::time::timeout(Duration::from_secs(5), &mut active.task).await.unwrap();
        host.completed(active, result).await;
        let next = frames(&mut rx);
        assert_eq!(next[0]["id"], "next");
        assert_eq!(next[0]["success"], true);
        assert!(next.iter().any(|frame| frame["type"] == "agent_start"));
        assert_eq!(next.iter().filter(|frame| frame["type"] == "agent_end").count(), 1);
        assert!(
            !next.iter().any(|frame| frame["error"].as_str().is_some_and(|error| error.contains("already running")))
        );
    }
}
