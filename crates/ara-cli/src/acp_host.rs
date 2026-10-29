//! Production ACP Host adapter. Fixed OMP 596f2da acp-mode/acp-agent/client-bridge.
//! The Native Host owns Agent execution, persistence, recovery and effect joins.
// MIT License
// Copyright (c) 2025 Mario Zechner
// Copyright (c) 2025-2026 Can Bölük
// Copyright (c) 2026 Stencil Labs, Inc.
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.
use super::{
    acp_plan::AcpPlan,
    acp_transport::{AcpConnection, AcpRpcError, Incoming},
    rpc_host,
};
use anyhow::{Context as _, Result, bail};
use ara_agent::{AgentConfig, AgentMessage};
use ara_discovery::LoadedSkill;
use ara_session::SessionJournal;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

pub(super) struct NativeSeed {
    pub config: AgentConfig,
    pub skills: Vec<LoadedSkill>,
    pub messages: Vec<AgentMessage>,
    pub journal: Option<SessionJournal>,
    pub header: Value,
    pub max_time: Option<f64>,
    pub sessions: rpc_host::SessionFactory,
}
struct FileBridge {
    connection: Arc<AcpConnection>,
    session_id: String,
    read: bool,
    write: bool,
}
#[async_trait]
impl ara_tools::edit_host::EditClientBridge for FileBridge {
    // Fixed capability is frozen at initialize. A disconnected advertised
    // writer must fail its owned request, never fall through to a local write.
    fn can_write_text_file(&self) -> bool {
        self.write
    }
    fn can_read_text_file(&self) -> bool {
        self.read
    }
    async fn read_text_file(&self, path: &Path) -> std::io::Result<String> {
        let result = self
            .connection
            .request(
                "fs/read_text_file",
                json!({
                    "sessionId": self.session_id, "path": path,
                }),
            )
            .await
            .map_err(|error| std::io::Error::other(error.message))?;
        result["content"].as_str().map(str::to_owned).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "ACP readTextFile response has no text content")
        })
    }
    async fn write_text_file(&self, path: &Path, content: &str) -> std::io::Result<()> {
        self.connection
            .request(
                "fs/write_text_file",
                json!({
                    "sessionId":self.session_id,"path":path,"content":content,
                }),
            )
            .await
            .map(|_| ())
            .map_err(|error| std::io::Error::other(error.message))
    }
}
struct Record {
    id: String,
    cwd: PathBuf,
    plan: Arc<AcpPlan>,
    plan_enabled: bool,
    handle: rpc_host::NativeSessionHandle,
    runtime: AsyncMutex<Option<rpc_host::NativeSessionRuntime>>,
    /// Only prompt lifetimes serialize here; cancel stays independently live.
    prompts: AsyncMutex<()>,
    /// Start/mode/cancel/close admission. It is released while a Run executes.
    controls: AsyncMutex<()>,
    closed: AtomicBool,
    error: Arc<Mutex<Option<String>>>,
    stop_reason: Arc<Mutex<String>>,
}
impl Record {
    fn check_open(&self) -> Result<()> {
        if self.closed.load(Ordering::Acquire) {
            bail!("ACP session closed: {}", self.id);
        }
        Ok(())
    }
    fn modes(&self) -> Value {
        mode_state(self.plan.current_mode(), self.plan_enabled)
    }
    fn options(&self) -> Value {
        mode_options(self.plan.current_mode(), self.plan_enabled)
    }
    async fn prompt(&self, params: &Value) -> Result<Value> {
        let (text, images) = prompt_content(params)?;
        let _prompt = self.prompts.lock().await;
        let admitted = {
            let _control = self.controls.lock().await;
            self.check_open()?;
            *self.error.lock().unwrap() = None;
            *self.stop_reason.lock().unwrap() = "end_turn".into();
            self.handle.admit(rpc_host::NativeAction::Prompt { text, images })?
        };
        // Admission is serialized with cancel's complete owned join, while
        // an admitted startup/preflight wait can still receive cancellation.
        admitted.await.context("Native prompt admission response closed")??;
        let settled = self.handle.call(rpc_host::NativeAction::WaitIdle).await?;
        let cancelled = settled["_nativeCancelled"] == true;
        if let Some(error) = self.error.lock().unwrap().clone() {
            if !(cancelled && error == "Request was aborted") {
                bail!(error);
            }
        }
        Ok(
            json!({"stopReason":if cancelled { "cancelled".to_owned() } else { self.stop_reason.lock().unwrap().clone() }}),
        )
    }
    async fn cancel(&self) -> Result<()> {
        let _control = self.controls.lock().await;
        self.check_open()?;
        self.handle.call(rpc_host::NativeAction::Abort).await?;
        *self.stop_reason.lock().unwrap() = "cancelled".into();
        Ok(())
    }
    async fn set_mode(&self, mode: &str) -> Result<Value> {
        {
            let _control = self.controls.lock().await;
            self.check_open()?;
            if mode != "default" && !(mode == "plan" && self.plan_enabled) {
                bail!("Unsupported ACP mode: {mode}");
            }
            // An explicit transition after cancel is allowed only after its
            // owned join. Don't retain the control gate while a proposal's
            // transition waits for a client approval: cancel must stay live.
            self.handle.permit_policy_refresh();
        }
        self.plan.set_mode(mode).await.map_err(anyhow::Error::msg)?;
        Ok(json!({}))
    }
    async fn close(&self) -> Result<()> {
        let _control = self.controls.lock().await;
        if self.closed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        self.handle.close_admission();
        if let Some(runtime) = self.runtime.lock().await.take() {
            let exit = runtime.close().await?;
            if exit != 0 {
                bail!("ACP Session persistence failed; restart from its journal");
            }
        }
        Ok(())
    }
    async fn replay_history(&self, connection: &AcpConnection) -> Result<()> {
        let _prompt = self.prompts.lock().await;
        let _control = self.controls.lock().await;
        self.check_open()?;
        let history = self.handle.call(rpc_host::NativeAction::Messages).await?;
        let messages = history["messages"].as_array().context("Native Session history missing")?;
        let mut calls = std::collections::HashSet::new();
        for message in messages {
            replay_message(connection, &self.id, message, &mut calls)?;
        }
        Ok(())
    }
}
struct Registry {
    connection: Arc<AcpConnection>,
    capabilities: Mutex<Option<Value>>,
    records: AsyncMutex<HashMap<String, Arc<Record>>>,
    /// Serializes preparation/adoption of the same saved identity, not Run IO.
    creation: AsyncMutex<()>,
    launch: AsyncMutex<NativeSeed>,
    connection_cancel: CancellationToken,
    closing: AtomicBool,
}
impl Registry {
    fn capabilities(&self) -> Result<Value> {
        self.capabilities.lock().unwrap().clone().context("ACP initialize must precede Session requests")
    }
    async fn record(&self, params: &Value) -> Result<Arc<Record>> {
        let id = string(params, "sessionId")?;
        self.records.lock().await.get(id).cloned().with_context(|| format!("ACP session not found: {id}"))
    }
    async fn open(&self, params: &Value, load: bool, replay: bool) -> Result<Value> {
        self.capabilities()?;
        let cwd = checked_cwd(params)?;
        let id = if load { Some(string(params, "sessionId")?.to_owned()) } else { None };
        let _creation = self.creation.lock().await;
        if self.closing.load(Ordering::Acquire) || self.connection.is_closed() {
            bail!("ACP connection closing");
        }
        if params.get("mcpServers").is_some_and(|servers| !servers.as_array().is_some_and(Vec::is_empty)) {
            bail!("ACP-provided MCP servers are not implemented in this batch");
        }
        if let Some(id) = &id {
            let existing = self.records.lock().await.get(id).cloned();
            if let Some(record) = existing {
                record.check_open()?;
                if lexical_cwd(&record.cwd)? != lexical_cwd(&cwd)? {
                    bail!("ACP session cwd does not match: {id}");
                }
                // Adoption is settled. Replay may wait behind a held Run;
                // EOF teardown must be able to acquire creation and close
                // this Native owner's independent connection immediately.
                drop(_creation);
                if replay {
                    record.replay_history(&self.connection).await?;
                }
                return Ok(json!({"modes":record.modes(),"configOptions":record.options()}));
            }
        }
        let launch = self.launch.lock().await;
        let stored = match id.as_deref() {
            Some(id) => Some(find_stored_session(id, &cwd, launch.sessions.args.session_dir.as_deref())?),
            None => None,
        };
        let seed = Box::pin(super::prepare_acp_seed(&launch.sessions, &cwd, stored, &self.connection_cancel)).await?;
        drop(launch);
        let session_id = seed.header["id"].as_str().context("Native Session identity missing")?.to_owned();
        if id.as_ref().is_some_and(|id| id != &session_id) {
            bail!("ACP stored Session identity mismatch");
        }
        let record = self.register(seed).await?;
        self.records.lock().await.insert(record.id.clone(), record.clone());
        drop(_creation);
        let response = if load {
            json!({"modes":record.modes(),"configOptions":record.options()})
        } else {
            json!({"sessionId":record.id,"modes":record.modes(),"configOptions":record.options()})
        };
        if replay {
            if let Err(error) = record.replay_history(&self.connection).await {
                let _ = record.close().await;
                let mut records = self.records.lock().await;
                if records.get(&record.id).is_some_and(|current| Arc::ptr_eq(current, &record)) {
                    records.remove(&record.id);
                }
                return Err(error);
            }
        }
        Ok(response)
    }
    async fn register(&self, mut seed: NativeSeed) -> Result<Arc<Record>> {
        let caps = self.capabilities()?;
        let id = seed.header["id"].as_str().context("Native Session identity missing")?.to_owned();
        let cwd = seed.sessions.cwd.clone();
        let edit = seed.sessions.tool_owner.edit_session.clone().context("Native EDIT Session owner missing")?;
        let local_root = edit.local_root().context("Native Session local mapping missing")?;
        let enabled =
            super::acp_plan::plan_enabled(&cwd, &super::ara_home().join("agent")).map_err(anyhow::Error::msg)?;
        let plan = AcpPlan::new(
            self.connection.clone(),
            id.clone(),
            cwd.clone(),
            local_root,
            !caps["elicitation"]["form"].is_null(),
        );
        plan.bind_config_options(Arc::new(move |mode| mode_options(mode, enabled)));
        let read = caps["fs"]["readTextFile"] == true;
        let write = caps["fs"]["writeTextFile"] == true;
        let bridge: Option<Arc<dyn ara_tools::edit_host::EditClientBridge>> = (read || write).then(|| {
            Arc::new(FileBridge { connection: self.connection.clone(), session_id: id.clone(), read, write }) as Arc<_>
        });
        plan.bind_uri_port(seed.sessions.uri_port.clone());
        edit.bind_protocol_services(bridge, plan.clone())?;
        seed.sessions.uri_port = Some(ara_cli::host_vault::compose_ports(plan.clone(), seed.sessions.uri_port.take()));
        seed.sessions.acp_plan = Some(plan.clone());
        let error = Arc::new(Mutex::new(None));
        let stop_reason = Arc::new(Mutex::new("end_turn".to_owned()));
        let sink_error = error.clone();
        let sink_reason = stop_reason.clone();
        let connection = self.connection.clone();
        let sink_id = id.clone();
        let frames = Arc::new(move |frame| native_wire_update(&connection, &sink_id, frame, &sink_error, &sink_reason));
        let runtime = rpc_host::spawn_native_session(
            seed.config,
            seed.skills,
            seed.messages,
            seed.journal,
            seed.header,
            seed.max_time,
            seed.sessions,
            frames,
        );
        let handle = runtime.handle.clone();
        plan.bind_refresh(handle.weak_refresh());
        plan.bind_session_close(handle.closed_signal());
        let state = handle.call(rpc_host::NativeAction::State).await;
        if let Err(error) = state {
            let _ = runtime.close().await;
            return Err(error);
        }
        Ok(Arc::new(Record {
            id,
            cwd,
            plan,
            plan_enabled: enabled,
            handle,
            runtime: AsyncMutex::new(Some(runtime)),
            prompts: AsyncMutex::new(()),
            controls: AsyncMutex::new(()),
            closed: AtomicBool::new(false),
            error,
            stop_reason,
        }))
    }
    async fn dispatch(&self, method: &str, params: Value) -> Result<Value> {
        if self.closing.load(Ordering::Acquire) || self.connection.is_closed() {
            bail!("ACP connection closing");
        }
        match method {
            "initialize" => {
                let mut caps = self.capabilities.lock().unwrap();
                if caps.is_some() {
                    bail!("ACP connection already initialized");
                }
                if params["protocolVersion"].as_u64() != Some(1) {
                    bail!("Unsupported ACP protocol version");
                }
                *caps = Some(params.get("clientCapabilities").cloned().unwrap_or_else(|| json!({})));
                Ok(
                    json!({"protocolVersion":1,"agentInfo":{"name":"ara","title":"ARA","version":env!("CARGO_PKG_VERSION")},
                    "authMethods":[{"id":"agent","name":"Use existing local credentials"}],
                    "agentCapabilities":{"loadSession":true,"sessionCapabilities":{"resume":{},"close":{}},
                    "promptCapabilities":{"image":false,"embeddedContext":false}}}),
                )
            }
            "authenticate" => {
                self.capabilities()?;
                if string(&params, "methodId")? != "agent" {
                    bail!("Unknown ACP auth method");
                }
                Ok(json!({}))
            }
            "session/new" => self.open(&params, false, false).await,
            "session/load" => self.open(&params, true, true).await,
            "session/resume" => self.open(&params, true, false).await,
            "session/set_mode" => self.record(&params).await?.set_mode(string(&params, "modeId")?).await,
            "session/set_config_option" => {
                if string(&params, "configId")? != "mode" {
                    bail!("Unsupported ACP config option");
                }
                let record = self.record(&params).await?;
                record.set_mode(string(&params, "value")?).await?;
                Ok(json!({"configOptions":record.options()}))
            }
            "session/prompt" => self.record(&params).await?.prompt(&params).await,
            "session/cancel" => {
                self.record(&params).await?.cancel().await?;
                Ok(json!({}))
            }
            "session/close" => {
                let id = string(&params, "sessionId")?;
                let record = self.records.lock().await.get(id).cloned();
                if let Some(record) = record {
                    record.close().await?;
                    let mut records = self.records.lock().await;
                    if records.get(id).is_some_and(|current| Arc::ptr_eq(current, &record)) {
                        records.remove(id);
                    }
                }
                Ok(json!({}))
            }
            _ => bail!("Unsupported ACP method: {method}"),
        }
    }
    async fn close(&self) -> Result<()> {
        self.closing.store(true, Ordering::Release);
        self.connection_cancel.cancel();
        let _creation = self.creation.lock().await;
        let records = std::mem::take(&mut *self.records.lock().await);
        let mut jobs = tokio::task::JoinSet::new();
        for (_, record) in records {
            jobs.spawn(async move { record.close().await });
        }
        let mut failure = None;
        while let Some(result) = jobs.join_next().await {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    failure.get_or_insert(error);
                }
                Err(error) => {
                    failure.get_or_insert(anyhow::Error::new(error).context("ACP Session teardown"));
                }
            }
        }
        let launch = self.launch.lock().await;
        if let Some(lsp) = &launch.sessions.tool_owner.lsp_startup {
            if let Err(error) = lsp.shutdown().await {
                failure.get_or_insert(error.into());
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }
        Ok(())
    }
}
// Construct this owned dispatcher outside the caller's async poll frame.
// Other CLI modes retain their original Windows stack and lifecycle.
pub(super) fn run(launch: NativeSeed) -> std::pin::Pin<Box<impl std::future::Future<Output = Result<i32>>>> {
    Box::pin(run_owned(launch))
}
async fn run_owned(launch: NativeSeed) -> Result<i32> {
    let mut transport = super::acp_transport::start(tokio::io::stdin(), tokio::io::stdout()).await;
    let registry = Arc::new(Registry {
        connection: transport.connection.clone(),
        capabilities: Mutex::new(None),
        records: AsyncMutex::new(HashMap::new()),
        creation: AsyncMutex::new(()),
        launch: AsyncMutex::new(launch),
        connection_cancel: CancellationToken::new(),
        closing: AtomicBool::new(false),
    });
    let mut requests = tokio::task::JoinSet::new();
    while let Some(incoming) = transport.incoming.recv().await {
        let registry = registry.clone();
        requests.spawn(async move {
            match incoming {
                Incoming::Request { id, method, params } => {
                    let result = registry
                        .dispatch(&method, params)
                        .await
                        .map_err(|error| AcpRpcError::new(-32603, error.to_string()));
                    registry.connection.response(id, result).await.map_err(anyhow::Error::from)
                }
                Incoming::Notification { method, params } => {
                    registry.dispatch(&method, params).await?;
                    Ok(())
                }
            }
        });
        while let Some(result) = requests.try_join_next() {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => eprintln!("ara: ACP request: {error}"),
                Err(error) => eprintln!("ara: ACP request owner: {error}"),
            }
        }
    }
    // Reader has closed all admitted external pending requests before joining
    // Native effects. Every callback is still owned until its receipt settles.
    let close = registry.close().await;
    while let Some(result) = requests.join_next().await {
        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => eprintln!("ara: ACP request: {error}"),
            Err(error) => eprintln!("ara: ACP request owner: {error}"),
        }
    }
    let io = transport.join().await;
    close?;
    io?;
    Ok(0)
}
fn string<'a>(params: &'a Value, key: &str) -> Result<&'a str> {
    params.get(key).and_then(Value::as_str).with_context(|| format!("ACP requires string {key}"))
}
fn lexical_cwd(path: &Path) -> Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    let mut output = PathBuf::new();
    for part in absolute.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                output.pop();
            }
            part => output.push(part.as_os_str()),
        }
    }
    Ok(output)
}
fn checked_cwd(params: &Value) -> Result<PathBuf> {
    let cwd = PathBuf::from(string(params, "cwd")?);
    if !cwd.is_absolute() {
        bail!("ACP requires an absolute cwd");
    }
    if !cwd.is_dir() {
        bail!("ACP cwd is not an existing directory");
    }
    lexical_cwd(&cwd)
}
fn find_stored_session(id: &str, cwd: &Path, explicit: Option<&Path>) -> Result<SessionJournal> {
    let directory = explicit.map(Path::to_owned).unwrap_or_else(|| super::default_session_dir(cwd));
    if id.is_empty() || id.contains(['/', '\\']) || id.contains("..") {
        bail!("Invalid ACP Session id");
    }
    for entry in std::fs::read_dir(&directory).with_context(|| format!("ACP session not found: {id}"))? {
        let path = entry?.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("jsonl") {
            continue;
        }
        let journal = match SessionJournal::open_with_blob_directory(&path, &super::ara_blobs_directory()) {
            Ok(journal) => journal,
            Err(_) => continue,
        };
        if journal.session_id() == id {
            let recorded = journal.header()["cwd"].as_str().context("Stored ACP Session cwd missing")?;
            if lexical_cwd(Path::new(recorded))? != lexical_cwd(cwd)? {
                bail!("ACP session cwd does not match: {id}");
            }
            return Ok(journal);
        }
    }
    bail!("ACP session not found: {id}")
}
fn mode_state(mode: &str, enabled: bool) -> Value {
    let mut available = vec![json!({"id":"default","name":"Default","description":"Standard ACP headless mode"})];
    if enabled {
        available.push(
            json!({"id":"plan","name":"Plan","description":"Read-only planning with session-local plan approval"}),
        );
    }
    json!({"availableModes":available,"currentModeId":mode})
}
fn mode_options(mode: &str, enabled: bool) -> Value {
    let mut options = vec![json!({"value":"default","name":"Default"})];
    if enabled {
        options.push(json!({"value":"plan","name":"Plan"}));
    }
    json!([{"id":"mode","name":"Mode","category":"mode","type":"select","currentValue":mode,"options":options}])
}
fn prompt_content(params: &Value) -> Result<(String, Vec<ara_ai::ImageContent>)> {
    let blocks = params["prompt"].as_array().context("ACP prompt must be an array")?;
    let mut text = String::new();
    for block in blocks {
        match block["type"].as_str() {
            Some("text") => {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(string(block, "text")?);
            }
            _ => bail!("ACP prompt content type is not implemented in this batch"),
        }
    }
    Ok((text, Vec::new()))
}
/// Fixed acp-agent.ts:2245-2508: replay the settled CURRENT Session history,
/// never execute a saved tool call or resend a client-side filesystem effect.
fn replay_message(
    connection: &AcpConnection,
    id: &str,
    message: &Value,
    calls: &mut std::collections::HashSet<String>,
) -> Result<()> {
    let error = Mutex::new(None);
    let stop = Mutex::new("end_turn".to_owned());
    let message_id = uuid::Uuid::now_v7().to_string();
    let notify_content = |kind: &str, content: Value| {
        connection
            .notify_now(
                "session/update",
                json!({
                    "sessionId":id,"update":{"sessionUpdate":kind,"content":content,"messageId":message_id}
                }),
            )
            .map_err(anyhow::Error::from)
    };
    let role = message["role"].as_str().unwrap_or("");
    if role == "toolResult" {
        let (Some(call_id), Some(name)) = (message["toolCallId"].as_str(), message["toolName"].as_str()) else {
            return Ok(());
        };
        if !calls.contains(call_id) {
            let args = match message["details"]["path"].as_str().filter(|path| !path.is_empty()) {
                Some(path) => json!({"path":path}),
                None => json!({}),
            };
            native_update(
                connection,
                id,
                json!({"type":"tool_execution_start","toolCallId":call_id,"toolName":name,"args":args}),
                &error,
                &stop,
            );
        }
        native_update(
            connection,
            id,
            json!({"type":"tool_execution_end","toolCallId":call_id,"toolName":name,
            "isError":message["isError"],"result":{"content":message["content"],"details":message["details"],"errorMessage":message["errorMessage"]}}),
            &error,
            &stop,
        );
        return Ok(());
    }
    let assistant = role == "assistant";
    if !assistant
        && !matches!(
            role,
            "user" | "developer" | "custom" | "hookMessage" | "bashExecution" | "pythonExecution" | "compactionSummary"
        )
    {
        return Ok(());
    }
    let mut emitted = false;
    if let Some(content) = message["content"].as_array() {
        for item in content {
            match item["type"].as_str() {
                Some("text") => {
                    if let Some(text) = item["text"].as_str().filter(|text| !text.is_empty()) {
                        notify_content(
                            if assistant { "agent_message_chunk" } else { "user_message_chunk" },
                            json!({"type":"text","text":text}),
                        )?;
                        emitted = true;
                    }
                }
                Some("image") => {
                    if item["data"].is_string() && item["mimeType"].is_string() {
                        notify_content(
                            if assistant { "agent_message_chunk" } else { "user_message_chunk" },
                            json!({"type":"image","data":item["data"],"mimeType":item["mimeType"]}),
                        )?;
                        emitted = true;
                    }
                }
                Some("thinking") if assistant => {
                    if let Some(thinking) =
                        item["thinking"].as_str().map(str::trim).filter(|thinking| !thinking.is_empty())
                    {
                        notify_content("agent_thought_chunk", json!({"type":"text","text":thinking}))?;
                        emitted = true;
                    }
                }
                Some("toolCall" | "tool_use") if assistant => {
                    if let (Some(call_id), Some(name)) = (item["id"].as_str(), item["name"].as_str()) {
                        let args = if let Some(args) = item.get("arguments") {
                            match args.as_str() {
                                Some(text) => serde_json::from_str(text).unwrap_or_else(|_| args.clone()),
                                None => args.clone(),
                            }
                        } else {
                            item.get("input").cloned().unwrap_or_else(|| json!({}))
                        };
                        native_update(
                            connection,
                            id,
                            json!({"type":"tool_execution_start","toolCallId":call_id,"toolName":name,"args":args}),
                            &error,
                            &stop,
                        );
                        calls.insert(call_id.to_owned());
                        emitted = true;
                    }
                }
                _ => {}
            }
        }
    }
    if assistant && !emitted && message["stopReason"] != "aborted" {
        if let Some(text) = message["errorMessage"].as_str().filter(|text| !text.is_empty()) {
            notify_content("agent_message_chunk", json!({"type":"text","text":text}))?;
        }
    }
    if connection.is_closed() {
        bail!("ACP peer closed during Session history replay");
    }
    Ok(())
}
// ACP's current protocol consumes these fields. Other Native frames (including
// auto_compaction_end) have always been ignored by this adapter, while their
// complete exact receipt still reaches the Native sink.
fn native_wire_update(
    connection: &AcpConnection,
    id: &str,
    frame: ara_rpc::WireValue,
    error: &Mutex<Option<String>>,
    stop: &Mutex<String>,
) {
    use ara_rpc::WireValue;
    let kind = frame.get("type").and_then(WireValue::as_string);
    let keys: &[&str] = match kind.and_then(|kind| kind.to_utf8().ok()).as_deref() {
        Some("message_update") => &["type", "assistantMessageEvent"],
        Some("tool_execution_start") => &["type", "toolCallId", "toolName", "args"],
        Some("tool_execution_update") => &["type", "toolCallId", "partialResult"],
        Some("tool_execution_end") => &["type", "toolCallId", "isError", "result"],
        Some("response") if matches!(frame.get("success"), Some(WireValue::Bool(false))) => {
            &["type", "success", "error"]
        }
        Some("agent_end") => &["type", "messages"],
        _ => return,
    };
    let mut projected = WireValue::object(Vec::new());
    for key in keys {
        if let Some(value) = frame.get(key) {
            let selected = match *key {
                "assistantMessageEvent" => {
                    let mut event = WireValue::object(Vec::new());
                    if let Some(kind) = value.get("type").and_then(WireValue::as_string)
                        && (kind.equals_ascii("text_delta") || kind.equals_ascii("thinking_delta"))
                    {
                        event.insert("type", WireValue::String(kind.clone()));
                        if let Some(delta) = value.get("delta") {
                            event.insert("delta", delta.clone());
                        }
                    }
                    event
                }
                "messages" => {
                    let messages = value
                        .as_array()
                        .into_iter()
                        .flatten()
                        .rev()
                        .find(|message| {
                            message
                                .get("role")
                                .and_then(WireValue::as_string)
                                .is_some_and(|role| role.equals_ascii("assistant"))
                        })
                        .into_iter()
                        .map(|message| {
                            let mut selected = WireValue::object(Vec::new());
                            for key in ["role", "stopReason", "errorMessage"] {
                                if let Some(value) = message.get(key) {
                                    selected.insert(key, value.clone());
                                }
                            }
                            selected
                        })
                        .collect();
                    WireValue::Array(messages)
                }
                _ => value.clone(),
            };
            projected.insert(key, selected);
        }
    }
    match serde_json::from_str(&projected.stringify()) {
        Ok(projected) => native_update(connection, id, projected, error, stop),
        Err(cause) => {
            let message = format!("ACP consumed Native event fields are not representable: {cause}");
            *error.lock().unwrap() = Some(message.clone());
            connection.close(message);
        }
    }
}

fn native_update(
    connection: &AcpConnection,
    id: &str,
    frame: Value,
    error: &Mutex<Option<String>>,
    stop: &Mutex<String>,
) {
    let update = match frame["type"].as_str() {
        Some("message_update") => {
            let event = &frame["assistantMessageEvent"];
            match event["type"].as_str() {
                Some("text_delta") => {
                    Some(json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":event["delta"]}}))
                }
                Some("thinking_delta") => {
                    Some(json!({"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":event["delta"]}}))
                }
                _ => None,
            }
        }
        Some("tool_execution_start") => Some(
            json!({"sessionUpdate":"tool_call","toolCallId":frame["toolCallId"],"title":frame["toolName"],"kind":"other","status":"in_progress","rawInput":frame["args"]}),
        ),
        Some("tool_execution_update") | Some("tool_execution_end") => {
            let complete = frame["type"] == "tool_execution_end";
            let result = if complete { &frame["result"] } else { &frame["partialResult"] };
            let content = result["content"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|block| json!({"type":"content","content":block}))
                .collect::<Vec<_>>();
            Some(json!({"sessionUpdate":"tool_call_update","toolCallId":frame["toolCallId"],
                "status":if complete { if frame["isError"] == true { "failed" } else { "completed" } } else { "in_progress" },"content":content,"rawOutput":result}))
        }
        Some("response") if frame["success"] == false => {
            *error.lock().unwrap() = Some(frame["error"].as_str().unwrap_or("Native Run failed").to_owned());
            None
        }
        Some("agent_end") => {
            if let Some(message) = frame["messages"]
                .as_array()
                .and_then(|messages| messages.iter().rev().find(|message| message["role"] == "assistant"))
            {
                *stop.lock().unwrap() = match message["stopReason"].as_str() {
                    Some("aborted") => "cancelled",
                    Some("length") => "max_tokens",
                    _ => "end_turn",
                }
                .into();
                if let Some(message) = message["errorMessage"].as_str() {
                    *error.lock().unwrap() = Some(message.to_owned());
                }
            }
            None
        }
        _ => None,
    };
    if let Some(update) = update {
        if let Err(error) = connection.notify_now("session/update", json!({"sessionId":id,"update":update})) {
            connection.close(error.to_string());
        }
    }
}
