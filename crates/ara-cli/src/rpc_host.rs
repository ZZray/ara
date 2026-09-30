//! First production RPC host slice. Fixed OMP `rpc-mode.ts` at
//! 596f2da7101178214aa27a753529d15e6b7ad91d; complete command parity remains open.
//!
//! Input, serial commands, owned Runs and ordered stdout have distinct owners.
//! A Run keeps its original journal even if the caller stops waiting. Live
//! queries use completed event messages, never the Agent transcript lock.

use anyhow::{Context as _, Result, bail};
use ara_agent::{Agent, AgentConfig, AgentError, AgentEvent, AgentEventSink, QueueMode, RunEnd, RunReport};
use ara_ai::{ImageContent, Message, UserBlock, UserContent, UserMessage};
use ara_rpc::{
    WireValue,
    frame::*,
    input::{InputItem, RpcInputReader},
    writer::RpcOutput,
};
use ara_session::SessionJournal;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    future::pending,
    path::PathBuf,
    sync::{Arc, Mutex},
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
}

impl SessionFactory {
    async fn config(&self, current: &AgentConfig, reset: bool) -> Result<AgentConfig> {
        let setup = super::prepare_cli_setup(
            &self.args,
            &self.cwd,
            &current.model,
            &self.mcp_config,
            (!reset).then(|| current.tools.clone()),
        )
        .await?;
        let mut config = current.clone();
        config.system_prompt = setup.system_prompt;
        config.tools = setup.tools;
        if reset {
            config.provider = self.provider.build();
            config.hooks = setup.hooks;
        }
        Ok(config)
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
        let mut response = json!({"type":"response", "command":command.kind, "success":error.is_none()});
        if let Some(error) = error {
            response["error"] = json!(error);
        } else if let Some(data) = data {
            response["data"] = data;
        }
        let mut response = wire(response);
        if let (Some(id), WireValue::Object(fields)) = (&command.id, &mut response) {
            fields.push(("id".into(), id.clone()));
        }
        let negotiated =
            command.kind == "negotiate_protocol" && response.get("success") == Some(&WireValue::Bool(true));
        self.send(if negotiated { OutputItem::Negotiate(response) } else { OutputItem::Frame(response) });
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
        if ara_discovery::parse_skill_invocation(&text).is_some() {
            bail!("RPC Skill invocation is not implemented yet; use the line REPL");
        }
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
    journal: tokio::sync::Mutex<Option<SessionJournal>>,
    // Public completed messages only. A partial is kept separately and never
    // appended by get_messages while the provider is still producing it.
    messages: Mutex<Vec<Value>>,
    partial: Mutex<Option<Value>>,
    persistence_error: Mutex<Option<String>>,
    header: Value,
    file: Option<std::path::PathBuf>,
}

impl Session {
    fn new(journal: Option<SessionJournal>, header: Value, messages: &[Message]) -> Arc<Self> {
        let public_messages = messages
            .iter()
            .map(|message| AgentEvent::MessageEnd { message: message.clone() }.full()["message"].clone())
            .collect();
        Arc::new(Self {
            file: journal.as_ref().map(|journal| journal.path().into()),
            journal: tokio::sync::Mutex::new(journal),
            messages: Mutex::new(public_messages),
            partial: Mutex::new(None),
            persistence_error: Mutex::new(None),
            header,
        })
    }
}

struct RunSink {
    session: Arc<Session>,
    output: Output,
    cancel: CancellationToken,
    connection: CancellationToken,
    terminal: Mutex<Option<Value>>,
}

#[async_trait]
impl AgentEventSink for RunSink {
    async fn emit(&self, event: AgentEvent) {
        let public = event.full();
        match &event {
            AgentEvent::MessageEnd { message } => {
                if let Some(journal) = self.session.journal.lock().await.as_mut()
                    && let Err(error) = journal.append_message(message)
                {
                    *self.session.persistence_error.lock().unwrap() = Some(error.to_string());
                    // The loop awaits this sink before starting tools. A failed
                    // assistant receipt therefore cancels before new effects.
                    self.cancel.cancel();
                    self.connection.cancel();
                }
                self.session.messages.lock().unwrap().push(public["message"].clone());
                *self.session.partial.lock().unwrap() = None;
            }
            AgentEvent::MessageStart { message: Message::Assistant(_) } | AgentEvent::MessageUpdate { .. } => {
                *self.session.partial.lock().unwrap() = Some(public["message"].clone());
            }
            AgentEvent::AgentEnd { .. } => {
                *self.session.partial.lock().unwrap() = None;
                // Fixed AgentSession defers the wire terminal until the prompt
                // unwinds. The client may immediately start its next Run.
                *self.terminal.lock().unwrap() = Some(public);
                return;
            }
            _ => {}
        }
        self.output.frame(public);
    }
}

struct ActiveRun {
    cancel: CancellationToken,
    task: JoinHandle<std::result::Result<RunReport, AgentError>>,
    command: Command,
    sink: Arc<RunSink>,
}

struct Host {
    agent: Arc<Agent>,
    session: Arc<Session>,
    config: AgentConfig,
    max_time: Option<f64>,
    output: Output,
    connection: CancellationToken,
    active: Option<ActiveRun>,
    sessions: SessionFactory,
    // Deliberate abort keeps queued input visible but suppresses autonomous
    // resumption. A new explicit prompt/queue command permits another Run.
    drain_queues: bool,
}

impl Host {
    fn adopt(&mut self, journal: Option<SessionJournal>, header: Value, messages: Vec<Message>, config: AgentConfig) {
        // Preparation is complete and the old owned Run is joined. Commit the
        // replacement once; each settled RunSink retains its original journal.
        let session = Session::new(journal, header, &messages);
        let agent = Agent::new(config.clone(), messages);
        agent.set_steering_mode(self.agent.steering_mode());
        agent.set_follow_up_mode(self.agent.follow_up_mode());
        self.agent = agent;
        self.session = session;
        self.config = config;
        self.drain_queues = false;
    }

    async fn new_session(&mut self, parent: Option<&str>) -> Result<()> {
        self.abort().await;
        let config = self.sessions.config(&self.config, true).await?;
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
        self.adopt(journal, header, Vec::new(), config);
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
        let config = self.sessions.config(&self.config, !unchanged).await?;
        // The launch-selected route is currently the only available model.
        // Keep unavailable saved selections intact, as fixed switch's fallback
        // does; role/catalog/thinking restoration remains a mapped WIP gap.
        super::recover_session(&mut journal)?;
        let messages = journal.model_context();
        let header = journal.header().clone();
        self.adopt(Some(journal), header, messages, config);
        Ok(false)
    }

    fn start(&mut self, message: Option<Message>, command: Command) -> Result<()> {
        if self.session.persistence_error.lock().unwrap().is_some() {
            bail!("session persistence failed; restart from the journal before continuing");
        }
        if self.active.is_some() {
            bail!("Agent is already running; specify streamingBehavior: steer or followUp");
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
        });
        let agent = self.agent.clone();
        let run_cancel = cancel.clone();
        let run_sink = sink.clone();
        // The serial Host owns the slot before it can accept another command.
        // ACK was enqueued by the caller before this task can emit AgentStart.
        let task = tokio::spawn(async move {
            match message {
                Some(message) => agent.prompt_with_config(vec![message], config, run_cancel, run_sink).await,
                None => agent.continue_run_with_config(config, run_cancel, run_sink).await,
            }
        });
        self.active = Some(ActiveRun { cancel, task, command, sink });
        Ok(())
    }

    fn completed(
        &mut self,
        active: ActiveRun,
        result: std::result::Result<std::result::Result<RunReport, AgentError>, tokio::task::JoinError>,
    ) {
        // Both callers joined the task and removed the active slot. Publish
        // idle only after the Agent's running guard and transcript lock release.
        if let Some(terminal) = active.sink.terminal.lock().unwrap().take() {
            self.output.frame(terminal);
        }
        let persistence_error = self.session.persistence_error.lock().unwrap().clone();
        let error = match result {
            Ok(Ok(report)) => match report.end {
                RunEnd::Completed => None,
                RunEnd::Aborted => Some("Request was aborted".into()),
                RunEnd::Deadline => Some("Deadline exceeded".into()),
                RunEnd::ModelCallBudget => Some("Model call limit reached".into()),
                RunEnd::Error => self
                    .session
                    .messages
                    .lock()
                    .unwrap()
                    .last()
                    .and_then(|message| message["errorMessage"].as_str())
                    .map(str::to_owned)
                    .or_else(|| Some("Agent run failed".into())),
            },
            Ok(Err(error)) => Some(error.to_string()),
            Err(error) => Some(format!("Run failed; tool effects may be unknown: {error}")),
        };
        let error = persistence_error
            .map(|error| format!("Session persistence failed ({error}); restart from the journal"))
            .or(error);
        if let Some(error) = error {
            self.drain_queues = false;
            self.output.response(&active.command, None, Some(error));
        }
    }

    async fn abort(&mut self) {
        self.drain_queues = false;
        if let Some(mut active) = self.active.take() {
            // Covers the interval before Agent::enter installs its own token.
            active.cancel.cancel();
            self.agent.abort();
            let result = (&mut active.task).await;
            self.completed(active, result);
        }
    }

    fn reconcile_queues(&mut self) {
        if self.active.is_none()
            && self.drain_queues
            && self.agent.has_queued_messages()
            && !self.connection.is_cancelled()
        {
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
            "isStreaming":self.active.is_some(),"isCompacting":false,
            "steeringMode":mode_name(self.agent.steering_mode()),"followUpMode":mode_name(self.agent.follow_up_mode()),
            "interruptMode":"wait", "sessionId":self.session.header["id"],
            "autoCompactionEnabled":false,"fastModeEnabled":false,"fastModeActive":false,
            "tokensPerSecond":null,"messageCount":self.session.messages.lock().unwrap().len(),
            "queuedMessageCount":steering+follow_up,"todoPhases":[],"systemPrompt":self.config.system_prompt,
            "dumpTools":self.config.tools.iter().map(|tool| tool.definition()).collect::<Vec<_>>()
        });
        if let Some(file) = &self.session.file {
            state["sessionFile"] = json!(file);
        }
        state
    }

    async fn handle(&mut self, command: Command) {
        let result = self.execute(&command).await;
        if let Err(error) = result {
            self.output.response(&command, None, Some(error.to_string()));
        }
        self.reconcile_queues();
    }

    async fn execute(&mut self, command: &Command) -> Result<()> {
        match command.kind.as_str() {
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
            "get_last_assistant_text" => {
                let text = last_assistant_text(&self.session.messages.lock().unwrap());
                let data = text.map_or_else(|| json!({}), |text| json!({"text":text}));
                self.output.response(command, Some(data), None);
            }
            "get_available_commands" => self.output.response(command, Some(json!({"commands":[]})), None),
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
                self.output.frame(json!({"type":"available_commands_update","commands":[]}));
                self.output.response(command, Some(json!({"cancelled":false})), None);
            }
            "switch_session" => {
                let path = PathBuf::from(command.string("sessionPath")?);
                let cancelled = self.switch_session(path).await?;
                if !cancelled {
                    self.output.frame(json!({"type":"available_commands_update","commands":[]}));
                }
                self.output.response(command, Some(json!({"cancelled":cancelled})), None);
            }
            "prompt" | "abort_and_prompt" => {
                let message = command.message()?;
                if command.kind == "abort_and_prompt" {
                    self.abort().await;
                }
                let queue = if let Some(value) = command.frame.get("streamingBehavior") {
                    match value.as_string().and_then(|s| s.to_utf8().ok()).as_deref() {
                        Some("steer") => Some(true),
                        Some("followUp") => Some(false),
                        _ => bail!("streamingBehavior must be steer or followUp"),
                    }
                } else {
                    None
                };
                self.output.response(command, None, None);
                self.drain_queues = true;
                if self.active.is_some()
                    && let Some(steer) = queue
                {
                    if steer {
                        self.agent.steer(message);
                    } else {
                        self.agent.follow_up(message);
                    }
                } else {
                    self.start(
                        Some(message),
                        Command { id: command.id.clone(), kind: command.kind.clone(), frame: command.frame.clone() },
                    )?;
                }
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
    messages: Vec<Message>,
    journal: Option<SessionJournal>,
    header: Value,
    max_time: Option<f64>,
    sessions: SessionFactory,
) -> Result<i32> {
    serve(tokio::io::stdin(), tokio::io::stdout(), config, messages, journal, header, max_time, sessions).await
}

#[allow(clippy::too_many_arguments)]
async fn serve<R, W>(
    input: R,
    writer: W,
    config: AgentConfig,
    messages: Vec<Message>,
    journal: Option<SessionJournal>,
    header: Value,
    max_time: Option<f64>,
    sessions: SessionFactory,
) -> Result<i32>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let connection = CancellationToken::new();
    let (output_tx, output_rx) = mpsc::unbounded_channel();
    let output = Output(output_tx);
    let output_task = tokio::spawn(write_output(writer, output_rx, connection.clone()));
    let session = Session::new(journal, header, &messages);
    let mut host = Host {
        agent: Agent::new(config.clone(), messages),
        session,
        config,
        max_time,
        output: output.clone(),
        connection: connection.clone(),
        active: None,
        sessions,
        drain_queues: true,
    };
    output.frame(json!({"type":"ready","protocolVersion":1,"supportedProtocolVersions":[1,2],
        "maxFrameBytes":MAX_RPC_FRAME_BYTES,"maxReassembledFrameBytes":MAX_RPC_REASSEMBLED_BYTES}));
    output.frame(json!({"type":"available_commands_update","commands":[]}));
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
                    if input_tx.send(Command::new(frame)).is_err() {
                        break;
                    }
                }
                Ok(Some(InputItem::ParseError(error))) => {
                    reader_output.frame(json!({"type":"response","command":"parse","success":false,"error":error}))
                }
                Ok(None) => break,
                Err(error) => {
                    reader_cancel.cancel();
                    return Err(error);
                }
            }
        }
        Ok(())
    });
    let mut eof = false;
    loop {
        host.reconcile_queues();
        if eof && host.active.is_none() {
            break;
        }
        tokio::select! {
            biased;
            _ = connection.cancelled() => { host.abort().await; break; }
            result = async {
                match &mut host.active {
                    Some(active) => (&mut active.task).await,
                    None => pending().await,
                }
            }, if host.active.is_some() => {
                let active = host.active.take().expect("selected active Run");
                host.completed(active, result);
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
    let failed = host.session.persistence_error.lock().unwrap().clone();
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

    #[tokio::test]
    async fn terminal_is_published_only_after_owned_run_settles() {
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
        let (tx, mut rx) = mpsc::unbounded_channel();
        let output = Output(tx);
        let connection = CancellationToken::new();
        let session = Session::new(None, super::super::ephemeral_header(&cwd, None), &[]);
        let mut host = Host {
            agent: Agent::new(config.clone(), Vec::new()),
            session: session.clone(),
            config: config.clone(),
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
            },
            drain_queues: false,
        };
        let cancel = connection.child_token();
        let sink =
            Arc::new(RunSink { session, output, cancel: cancel.clone(), connection, terminal: Mutex::new(None) });
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
            agent.prompt_with_config(vec![Message::User(UserMessage::text("first"))], config, run_cancel, gate).await
        });
        host.active =
            Some(ActiveRun { cancel, task, command: Command::new(wire(json!({"type":"prompt","id":"first"}))), sink });

        tokio::time::timeout(Duration::from_secs(5), reached).await.unwrap().unwrap();
        assert!(!host.active.as_ref().unwrap().task.is_finished());
        assert_eq!(host.state()["isStreaming"], true);
        let live = frames(&mut rx);
        assert!(live.iter().any(|frame| frame["type"] == "agent_start"));
        assert!(!live.iter().any(|frame| frame["type"] == "agent_end"));

        release.send(()).unwrap();
        let mut active = host.active.take().unwrap();
        let result = tokio::time::timeout(Duration::from_secs(5), &mut active.task).await.unwrap();
        host.completed(active, result);
        assert_eq!(host.state()["isStreaming"], false);
        let settled = frames(&mut rx);
        assert_eq!(settled.iter().filter(|frame| frame["type"] == "agent_end").count(), 1);
        assert_eq!(settled[0]["type"], "agent_end");
        assert_eq!(settled[1]["id"], "first");
        assert_eq!(settled[1]["error"], "Model call limit reached");

        host.handle(Command::new(wire(json!({"type":"prompt","id":"next","message":"next"})))).await;
        let mut active = host.active.take().expect("immediate next prompt starts");
        let result = tokio::time::timeout(Duration::from_secs(5), &mut active.task).await.unwrap();
        host.completed(active, result);
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
