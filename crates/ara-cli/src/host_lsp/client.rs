// Ported from Oh My Pi 596f2da7101178214aa27a753529d15e6b7ad91d, MIT.
// Copyright (c) 2025 Mario Zechner; 2025-2026 Can Bölük; 2026 Stencil Labs, Inc.
// The upstream MIT permission and disclaimer are reproduced in types.rs.

use super::{
    config::{self, LspConfig, ServerConfig},
    custom, diagnostics, mux,
    types::*,
};
use futures::future::join_all;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    io,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    io::AsyncReadExt,
    sync::{Mutex, mpsc, oneshot, watch},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub(super) struct Document {
    pub version: u64,
    pub language: String,
    pub synced_content: String,
}
pub(super) struct ClientState {
    pub capabilities: Value,
    pub dynamic: HashMap<String, String>,
    pub documents: HashMap<String, Document>,
    pub diagnostics: HashMap<String, PublishedDiagnostics>,
    pub diagnostic_version: u64,
    pub progress: HashSet<String>,
    pub project_loaded: bool,
    pub rust_workspace_ready: bool,
    pub activity: Instant,
}
struct Outgoing {
    message: Value,
    cancel: Option<CancellationToken>,
    receipt: Option<oneshot::Sender<io::Result<()>>>,
}
pub(super) struct Client {
    pub server: ServerConfig,
    pub cwd: PathBuf,
    pub state: StdMutex<ClientState>,
    pending: StdMutex<HashMap<String, oneshot::Sender<io::Result<Value>>>>,
    document_locks: StdMutex<HashMap<String, Arc<Mutex<()>>>>,
    writes: mpsc::UnboundedSender<Outgoing>,
    request_id: AtomicU64,
    pub stop: CancellationToken,
    tasks: StdMutex<Vec<JoinHandle<()>>>,
    exited: watch::Receiver<bool>,
    stderr: Arc<StdMutex<String>>,
    pub shared: bool,
    presence: Option<Arc<mux::Presence>>,
}
impl Client {
    fn finish(&self, message: &str) {
        self.stop.cancel();
        self.state.lock().unwrap().project_loaded = true;
        for (_, pending) in self.pending.lock().unwrap().drain() {
            let _ = pending.send(Err(error(message)));
        }
    }
    pub fn alive(&self) -> bool {
        !self.stop.is_cancelled() && !*self.exited.borrow()
    }
    fn document_lock(&self, uri: &str) -> Arc<Mutex<()>> {
        self.document_locks.lock().unwrap().entry(uri_key(uri)).or_insert_with(|| Arc::new(Mutex::new(()))).clone()
    }
    async fn start(
        server: ServerConfig,
        cwd: PathBuf,
        agent_dir: &Path,
        shared: bool,
        lifetime: &CancellationToken,
        creation_cancel: &CancellationToken,
        owned_clients: &StdMutex<Vec<Arc<Client>>>,
        cancel: Option<CancellationToken>,
    ) -> io::Result<Arc<Self>> {
        check_cancel(cancel.as_ref())?;
        let stop = lifetime.child_token();
        let wrapped = mux::wrapped_command(&server.resolved_command, &server.args, &cwd, cancel.as_ref()).await?;
        let mut env = server.env.clone();
        env.extend(wrapped.env);
        let transport = if shared && !wrapped.wrapped {
            mux::connect_shared(&wrapped.command, &wrapped.args, &cwd, agent_dir, &env, lifetime, cancel.as_ref())
                .await?
        } else {
            None
        };
        check_cancel(cancel.as_ref())?;
        let stderr = Arc::new(StdMutex::new(String::new()));
        let (exit_tx, exited) = watch::channel(false);
        let mut process = None;
        let (reader, writer, is_shared, presence) = match transport {
            Some(transport) => (transport.reader, transport.writer, true, Some(transport.presence)),
            None => {
                let mut child_command = custom::command(&wrapped.command);
                child_command
                    .args(&wrapped.args)
                    .envs(&env)
                    .current_dir(&cwd)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
                let mut child = custom::spawn(child_command)?;
                let stdout = child.stdout.take().ok_or_else(|| error("missing LSP stdout"))?;
                let stdin = child.stdin.take().ok_or_else(|| error("missing LSP stdin"))?;
                let child_stderr = child.stderr.take();
                process = Some((child, child_stderr));
                (FramedReader::new(Box::new(stdout)), Box::new(stdin) as WriteStream, false, None)
            }
        };
        let (writes, rx) = mpsc::unbounded_channel();
        let client = Arc::new(Self {
            server,
            cwd,
            state: StdMutex::new(ClientState {
                capabilities: Value::Null,
                dynamic: HashMap::new(),
                documents: HashMap::new(),
                diagnostics: HashMap::new(),
                diagnostic_version: 0,
                progress: HashSet::new(),
                project_loaded: false,
                rust_workspace_ready: false,
                activity: Instant::now(),
            }),
            pending: StdMutex::new(HashMap::new()),
            document_locks: StdMutex::new(HashMap::new()),
            writes,
            request_id: AtomicU64::new(0),
            stop,
            tasks: StdMutex::new(Vec::new()),
            exited,
            stderr,
            shared: is_shared,
            presence,
        });
        // Include mid-initialize clients in explicit Pool shutdown settlement.
        owned_clients.lock().unwrap().push(client.clone());
        struct InitGuard(Option<Arc<Client>>);
        impl Drop for InitGuard {
            fn drop(&mut self) {
                if let Some(client) = &self.0 {
                    client.finish("LSP initialization did not complete");
                }
            }
        }
        let mut guard = InitGuard(Some(client.clone()));
        if let Some((mut child, child_stderr)) = process {
            if let Some(mut stderr_pipe) = child_stderr {
                let stop = client.stop.clone();
                let buffer = client.stderr.clone();
                client.tasks.lock().unwrap().push(tokio::spawn(async move {
                    let mut chunk = [0; 4096];
                    loop {
                        let read = tokio::select! { read = stderr_pipe.read(&mut chunk) => read, _ = stop.cancelled() => break };
                        match read {
                            Ok(0) | Err(_) => break,
                            Ok(size) => { let mut text = buffer.lock().unwrap(); text.push_str(&String::from_utf8_lossy(&chunk[..size])); if text.len() > 8192 { let offset = text.char_indices().map(|(i, _)| i).find(|i| *i >= text.len() - 8192).unwrap_or(0); text.drain(..offset); } },
                        }
                    }
                }));
            }
            let owned = client.clone();
            client.tasks.lock().unwrap().push(tokio::spawn(async move {
                let status = tokio::select! {
                    status = child.wait() => status.map(|_|()),
                    _ = owned.stop.cancelled() => Ok(())
                };
                let detail = owned.stderr.lock().unwrap().clone();
                owned.finish(&format!("LSP server exited ({status:?}): {}", detail.trim()));
                let tree_confirmed = custom::terminate(&mut child).await.is_ok();
                let _ = exit_tx.send(status.is_ok() && tree_confirmed);
            }));
        } else {
            let owned = client.clone();
            client.tasks.lock().unwrap().push(tokio::spawn(async move {
                owned.stop.cancelled().await;
                let _ = exit_tx.send(true);
            }));
        }
        let owned = client.clone();
        client.tasks.lock().unwrap().push(tokio::spawn(async move {
            owned.writer(writer, rx).await;
        }));
        let owned = client.clone();
        client.tasks.lock().unwrap().push(tokio::spawn(async move {
            owned.reader(reader).await;
        }));
        let weak = Arc::downgrade(&client);
        let stop = client.stop.clone();
        client.tasks.lock().unwrap().push(tokio::spawn(async move {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(15)) => { if let Some(client) = weak.upgrade() { client.state.lock().unwrap().project_loaded = true; } },
                _ = stop.cancelled() => {},
            }
        }));
        let init = async {
            let result = client.request("initialize", json!({"processId":std::process::id(),"rootUri":file_uri(&client.cwd),"rootPath":client.cwd,"capabilities":client_capabilities(),"initializationOptions":client.server.init_options,"workspaceFolders":[{"uri":file_uri(&client.cwd),"name":client.cwd.file_name().map(|v| v.to_string_lossy()).unwrap_or_default()}]}), cancel.clone(), None).await?;
            if !result.is_object() {
                return Err(error("Failed to initialize LSP: no response"));
            }
            client.state.lock().unwrap().capabilities = result.get("capabilities").cloned().unwrap_or(Value::Null);
            client.notify("initialized", json!({}), cancel.clone()).await?;
            client
                .notify("workspace/didChangeConfiguration", json!({"settings":client.server.settings}), cancel.clone())
                .await?;
            Ok::<_, io::Error>(())
        };
        tokio::select! { result = init => result?, _ = creation_cancel.cancelled() => return Err(cancelled()) }
        guard.0 = None;
        Ok(client)
    }
    async fn writer(self: Arc<Self>, mut output: WriteStream, mut rx: mpsc::UnboundedReceiver<Outgoing>) {
        loop {
            let packet = tokio::select! { packet = rx.recv() => match packet { Some(packet) => packet, None => break }, _ = self.stop.cancelled() => break };
            if let Err(error) = check_cancel(packet.cancel.as_ref()) {
                if let Some(receipt) = packet.receipt {
                    let _ = receipt.send(Err(error));
                }
                continue;
            }
            let result = tokio::select! {
                result = write_frame(&mut output, &packet.message) => result,
                _ = cancellation(packet.cancel.as_ref()) => { self.finish("LSP write drain aborted; client torn down"); Err(cancelled()) },
                _ = self.stop.cancelled() => Err(error("LSP transport closed")),
            };
            let failed = result.is_err();
            if let Some(receipt) = packet.receipt {
                let _ = receipt.send(result);
            }
            if failed {
                self.finish("LSP write failed; client torn down");
                break;
            }
        }
        while let Ok(packet) = rx.try_recv() {
            if let Some(receipt) = packet.receipt {
                let _ = receipt.send(Err(error("LSP writer stopped")));
            }
        }
    }
    async fn reader(self: Arc<Self>, mut reader: FramedReader) {
        loop {
            let message = tokio::select! { message = reader.next() => message, _ = self.stop.cancelled() => return };
            let message = match message {
                Ok(Some(message)) => message,
                Ok(None) => break,
                Err(error) => {
                    self.finish(&format!("LSP reader failed: {error}"));
                    return;
                }
            };
            // Methods precede response matching: server request ids can collide.
            if let Some(method) = message.get("method").and_then(Value::as_str) {
                if message.get("id").is_some() {
                    if let Err(error) = self.server_request(&message).await {
                        eprintln!("LSP server request failed: {error}");
                    }
                } else {
                    let params = &message["params"];
                    let mut state = self.state.lock().unwrap();
                    if method == "textDocument/publishDiagnostics" {
                        if let (Some(uri), Some(items)) = (
                            params["uri"].as_str(),
                            params
                                .get("diagnostics")
                                .cloned()
                                .and_then(|v| serde_json::from_value::<Vec<Diagnostic>>(v).ok()),
                        ) {
                            state.diagnostic_version += 1;
                            let publication = state.diagnostic_version;
                            state.diagnostics.insert(
                                uri_key(uri),
                                PublishedDiagnostics {
                                    items,
                                    document_version: params["version"].as_u64(),
                                    publication,
                                },
                            );
                        }
                    } else if method == "$/progress" {
                        let token = params["token"].to_string();
                        match params.pointer("/value/kind").and_then(Value::as_str) {
                            Some("begin") => {
                                state.progress.insert(token);
                            }
                            Some("end") => {
                                state.progress.remove(&token);
                                if state.progress.is_empty() {
                                    state.project_loaded = true;
                                }
                            }
                            _ => {}
                        }
                    }
                }
            } else if let Some(id) = message.get("id") {
                let pending = self.pending.lock().unwrap().remove(&id.to_string());
                if let Some(pending) = pending {
                    self.state.lock().unwrap().activity = Instant::now();
                    let result = if let Some(problem) = message.get("error").filter(|v| !v.is_null()) {
                        Err(error(format!(
                            "LSP error {}: {}",
                            problem["code"],
                            problem["message"].as_str().unwrap_or("unknown")
                        )))
                    } else {
                        Ok(message.get("result").cloned().unwrap_or(Value::Null))
                    };
                    let _ = pending.send(result);
                }
            }
        }
        if !self.shared {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        self.finish("LSP reader stopped; client torn down");
    }
    async fn server_request(&self, message: &Value) -> io::Result<()> {
        let method = message["method"].as_str().unwrap_or_default();
        let params = &message["params"];
        let mut problem = None;
        let result = match method {
            "workspace/configuration" => Value::Array(
                params["items"]
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .map(|item| {
                                self.server
                                    .settings
                                    .get(item["section"].as_str().unwrap_or_default())
                                    .cloned()
                                    .unwrap_or(Value::Null)
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            ),
            "workspace/workspaceFolders" => {
                json!([{"uri":file_uri(&self.cwd),"name":self.cwd.file_name().map(|v|v.to_string_lossy()).unwrap_or_default()}])
            }
            // Host grants and Session effects cannot be retained on a shared client.
            "workspace/applyEdit" => json!({"applied":false,"failureReason":"Host workspace edit adapter unavailable"}),
            "client/registerCapability" | "client/unregisterCapability" => {
                let mut state = self.state.lock().unwrap();
                if method == "client/registerCapability" {
                    if let Some(items) = params["registrations"].as_array() {
                        for item in items {
                            if let (Some(id), Some(method)) = (item["id"].as_str(), item["method"].as_str()) {
                                state.dynamic.insert(id.into(), method.into());
                            }
                        }
                    }
                } else if let Some(items) =
                    params.get("unregisterations").or_else(|| params.get("unregistrations")).and_then(Value::as_array)
                {
                    for item in items {
                        if let Some(id) = item["id"].as_str() {
                            state.dynamic.remove(id);
                        }
                    }
                }
                Value::Null
            }
            "window/showDocument" => json!({"success":false}),
            "window/showMessageRequest"
            | "window/workDoneProgress/create"
            | "workspace/semanticTokens/refresh"
            | "workspace/inlayHint/refresh"
            | "workspace/codeLens/refresh"
            | "workspace/codeAction/refresh"
            | "workspace/inlineValue/refresh"
            | "workspace/foldingRange/refresh"
            | "workspace/diagnostic/refresh" => Value::Null,
            _ => {
                problem = Some(json!({"code":-32601,"message":format!("Method not found: {method}")}));
                Value::Null
            }
        };
        let response = if let Some(problem) = problem {
            json!({"jsonrpc":"2.0","id":message["id"],"error":problem})
        } else {
            json!({"jsonrpc":"2.0","id":message["id"],"result":result})
        };
        self.send(response, None).await
    }
    async fn send(&self, message: Value, cancel: Option<CancellationToken>) -> io::Result<()> {
        check_cancel(cancel.as_ref())?;
        if self.stop.is_cancelled() {
            return Err(error("LSP transport closed"));
        }
        let (tx, rx) = oneshot::channel();
        self.writes
            .send(Outgoing { message, cancel: cancel.clone(), receipt: Some(tx) })
            .map_err(|_| error("LSP writer stopped"))?;
        tokio::select! { result = rx => result.map_err(|_| error("LSP writer lost receipt"))?, _ = cancellation(cancel.as_ref()) => Err(cancelled()), _ = self.stop.cancelled() => Err(error("LSP transport closed")) }
    }
    pub async fn notify(&self, method: &str, params: Value, cancel: Option<CancellationToken>) -> io::Result<()> {
        self.state.lock().unwrap().activity = Instant::now();
        self.send(json!({"jsonrpc":"2.0","method":method,"params":params}), cancel).await
    }
    pub async fn request(
        &self,
        method: &str,
        params: Value,
        cancel: Option<CancellationToken>,
        timeout_ms: Option<u64>,
    ) -> io::Result<Value> {
        check_cancel(cancel.as_ref())?;
        let id = self.request_id.fetch_add(1, Ordering::SeqCst) + 1;
        let key = id.to_string();
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(key.clone(), tx);
        struct PendingGuard<'a> {
            client: &'a Client,
            key: String,
            id: u64,
            completed: bool,
            drain: CancellationToken,
        }
        impl Drop for PendingGuard<'_> {
            fn drop(&mut self) {
                self.client.pending.lock().unwrap().remove(&self.key);
                self.client.state.lock().unwrap().activity = Instant::now();
                if !self.completed {
                    self.drain.cancel();
                    let _ = self.client.writes.send(Outgoing {
                        message: json!({"jsonrpc":"2.0","method":"$/cancelRequest","params":{"id":self.id}}),
                        cancel: None,
                        receipt: None,
                    });
                }
            }
        }
        let drain = CancellationToken::new();
        let mut guard = PendingGuard { client: self, key, id, completed: false, drain: drain.clone() };
        let operation = async {
            self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}), Some(drain)).await?;
            rx.await.map_err(|_| error("LSP request lost owner"))?
        };
        let deadline = timeout_ms.or_else(|| cancel.is_none().then_some(30_000));
        let timeout = async {
            match deadline {
                Some(ms) => tokio::time::sleep(Duration::from_millis(ms)).await,
                None => std::future::pending().await,
            }
        };
        let result = tokio::select! { result = operation => result, _ = timeout => Err(io::Error::new(io::ErrorKind::TimedOut, format!("LSP request {method} timed out after {}ms", deadline.unwrap_or_default()))), _ = cancellation(cancel.as_ref()) => Err(cancelled()), _ = self.stop.cancelled() => Err(error("LSP transport closed")) };
        guard.completed = !matches!(&result, Err(error) if matches!(error.kind(), io::ErrorKind::Interrupted | io::ErrorKind::TimedOut));
        result
    }
    pub async fn sync(&self, path: &Path, content: &str, cancel: Option<CancellationToken>) -> io::Result<()> {
        let uri = file_uri(path);
        let key = uri_key(&uri);
        let lock = self.document_lock(&uri);
        let _owner = tokio::select! { owner = lock.lock() => owner, _ = cancellation(cancel.as_ref()) => return Err(cancelled()) };
        check_cancel(cancel.as_ref())?;
        let previous = {
            let mut state = self.state.lock().unwrap();
            state.diagnostics.remove(&key);
            state.documents.get(&key).cloned()
        };
        let language = self.server.language_id.clone().unwrap_or_else(|| language_id(path));
        let version = previous.as_ref().map(|v| v.version + 1).unwrap_or(1);
        if previous.is_none() {
            self.notify(
                "textDocument/didOpen",
                json!({"textDocument":{"uri":uri,"languageId":language,"version":version,"text":content}}),
                cancel,
            )
            .await?;
        } else {
            self.notify(
                "textDocument/didChange",
                json!({"textDocument":{"uri":uri,"version":version},"contentChanges":[{"text":content}]}),
                cancel,
            )
            .await?;
        }
        self.state.lock().unwrap().documents.insert(key, Document { version, language, synced_content: content.to_owned() });
        Ok(())
    }
    pub async fn saved(&self, path: &Path, cancel: Option<CancellationToken>) -> io::Result<()> {
        let uri = file_uri(path);
        let lock = self.document_lock(&uri);
        let _owner = tokio::select! { owner = lock.lock() => owner, _ = cancellation(cancel.as_ref()) => return Err(cancelled()) };
        if !self.state.lock().unwrap().documents.contains_key(&uri_key(&uri)) {
            return Ok(());
        }
        self.notify("textDocument/didSave", json!({"textDocument":{"uri":uri}}), cancel).await
    }
    pub async fn wait_project(&self, cancel: Option<CancellationToken>) -> io::Result<()> {
        loop {
            check_cancel(cancel.as_ref())?;
            if self.state.lock().unwrap().project_loaded {
                break;
            }
            tokio::select! { _ = tokio::time::sleep(Duration::from_millis(100)) => {}, _ = cancellation(cancel.as_ref()) => return Err(cancelled()), _ = self.stop.cancelled() => return Err(error("LSP project load interrupted by exit")) }
        }
        if self.server.command.split(['/', '\\']).next_back() == Some("rust-analyzer")
            && !self.state.lock().unwrap().rust_workspace_ready
        {
            let timing = &self.server.workspace_ready_timings;
            let timeout = timing["timeoutMs"].as_u64().unwrap_or(5000);
            let poll = timing["pollMs"].as_u64().unwrap_or(100);
            let settle = timing["settleMs"].as_u64().unwrap_or(2000);
            let request = timing["statusRequestTimeoutMs"].as_u64().unwrap_or(1000);
            let start = Instant::now();
            loop {
                check_cancel(cancel.as_ref())?;
                match self.request("rust-analyzer/analyzerStatus", json!({}), cancel.clone(), Some(request)).await {
                    Ok(status)
                        if status.as_str().is_some_and(|v| !v.starts_with("No workspaces"))
                            && start.elapsed().as_millis() >= u128::from(settle) =>
                    {
                        self.state.lock().unwrap().rust_workspace_ready = true;
                        break;
                    }
                    Err(error) if error.kind() != io::ErrorKind::TimedOut => break,
                    _ => {}
                }
                if start.elapsed().as_millis() >= u128::from(timeout) {
                    break;
                }
                tokio::select! { _ = tokio::time::sleep(Duration::from_millis(poll)) => {}, _ = cancellation(cancel.as_ref()) => return Err(cancelled()) }
            }
        }
        check_cancel(cancel.as_ref())
    }
    async fn shutdown(&self) -> io::Result<()> {
        for (_, pending) in self.pending.lock().unwrap().drain() {
            let _ = pending.send(Err(error("LSP client shutdown")));
        }
        if self.alive() && self.request("shutdown", Value::Null, None, Some(5000)).await.is_ok() {
            let _ = tokio::time::timeout(Duration::from_secs(1), self.notify("exit", Value::Null, None)).await;
            let mut exited = self.exited.clone();
            let _ = tokio::time::timeout(Duration::from_secs(1), async {
                while !*exited.borrow() {
                    if exited.changed().await.is_err() {
                        break;
                    }
                }
            })
            .await;
        }
        self.finish("LSP client shutdown");
        let tasks = std::mem::take(&mut *self.tasks.lock().unwrap());
        // Never drop JoinHandles under an outer timeout: abort and await each
        // lingering owner so readers/pending queues cannot detach from teardown.
        let results = join_all(tasks.into_iter().map(|mut task| async move {
            if tokio::time::timeout(Duration::from_secs(7), &mut task).await.is_err() {
                task.abort();
                let _ = task.await;
                false
            } else {
                true
            }
        }))
        .await;
        if results.iter().any(|value| !*value) || !*self.exited.borrow() {
            return Err(error("LSP process exit was not confirmed"));
        }
        Ok(())
    }
}

struct Slot {
    client: Option<Arc<Client>>,
    failure: Option<(Instant, String)>,
}
struct PendingDiskWriteLease {
    pending: Arc<StdMutex<HashMap<String, usize>>>,
    key: String,
}
impl Drop for PendingDiskWriteLease {
    fn drop(&mut self) {
        let mut pending = self.pending.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(count) = pending.get_mut(&self.key) {
            *count -= 1;
            if *count == 0 { pending.remove(&self.key); }
        }
    }
}
#[derive(Clone)]
pub(super) struct PendingDiskWrite(Arc<PendingDiskWriteLease>);

pub struct ClientPool {
    cwd: PathBuf,
    agent_dir: PathBuf,
    config: LspConfig,
    shared: bool,
    slots: Mutex<HashMap<String, Arc<Mutex<Slot>>>>,
    closing: AtomicBool,
    lifetime: CancellationToken,
    creation_cancel: CancellationToken,
    presences: StdMutex<Vec<Arc<mux::Presence>>>,
    owned_clients: StdMutex<Vec<Arc<Client>>>,
    pending_disk_writes: Arc<StdMutex<HashMap<String, usize>>>,
}
impl Drop for ClientPool {
    fn drop(&mut self) {
        self.creation_cancel.cancel();
        self.lifetime.cancel();
    }
}
impl ClientPool {
    pub fn new(cwd: PathBuf, agent_dir: PathBuf, config: LspConfig, shared: bool) -> Arc<Self> {
        let pool = Arc::new(Self {
            cwd: absolute(&cwd),
            agent_dir: absolute(&agent_dir),
            config,
            shared,
            slots: Mutex::new(HashMap::new()),
            closing: AtomicBool::new(false),
            lifetime: CancellationToken::new(),
            creation_cancel: CancellationToken::new(),
            presences: StdMutex::new(Vec::new()),
            owned_clients: StdMutex::new(Vec::new()),
            pending_disk_writes: Arc::new(StdMutex::new(HashMap::new())),
        });
        if let Some(timeout) = pool.config.idle_timeout_ms.filter(|v| *v > 0) {
            let weak = Arc::downgrade(&pool);
            let lifetime = pool.lifetime.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! { _ = tokio::time::sleep(Duration::from_secs(60)) => {}, _ = lifetime.cancelled() => break }
                    let Some(pool) = weak.upgrade() else {
                        break;
                    };
                    let slots = pool.slots.lock().await.values().cloned().collect::<Vec<_>>();
                    for slot in slots {
                        let mut slot = slot.lock().await;
                        let idle = slot.client.as_ref().is_some_and(|client| {
                            client.pending.lock().unwrap().is_empty()
                                && client.state.lock().unwrap().activity.elapsed().as_millis() > u128::from(timeout)
                        });
                        if idle {
                            if let Some(client) = slot.client.take() {
                                if let Err(error) = client.shutdown().await {
                                    eprintln!("LSP idle shutdown failed: {error}");
                                    slot.client = Some(client);
                                }
                            }
                        }
                    }
                }
            });
        }
        pool
    }
    pub(super) fn begin_pending_disk_write(&self, path: &Path) -> PendingDiskWrite {
        let key = uri_key(&file_uri(path));
        *self.pending_disk_writes.lock().unwrap_or_else(|error| error.into_inner()).entry(key.clone()).or_default() += 1;
        PendingDiskWrite(Arc::new(PendingDiskWriteLease { pending: self.pending_disk_writes.clone(), key }))
    }
    fn disk_write_pending(&self, key: &str) -> bool {
        self.pending_disk_writes.lock().unwrap_or_else(|error| error.into_inner()).contains_key(key)
    }

    /// New files exist and watched notifications settle before the first open.
    /// Only advertised TypeScript servers get this project-reload request.
    pub(super) async fn reload_typescript_after_create(
        &self, servers: &[ServerConfig], cancel: Option<CancellationToken>, create_missing: bool,
    ) -> io::Result<()> {
        let results = join_all(servers.iter().filter(|server| !server.custom).map(|server| {
            let cancel = cancel.clone();
            async move {
                let client = match self.get(server, cancel.clone(), create_missing).await {
                    Ok(Some(client)) => client,
                    Ok(None) => return Ok(()),
                    Err(_) => { check_cancel(cancel.as_ref())?; return Ok(()); }
                };
                let supported = client.state.lock().unwrap().capabilities
                    .get("executeCommandProvider").and_then(|value| value.get("commands"))
                    .and_then(Value::as_array)
                    .is_some_and(|commands| commands.iter().any(|command| command == "typescript.tsserverRequest"));
                if !supported { return Ok(()); }
                let response = client.request("workspace/executeCommand", json!({
                    "command":"typescript.tsserverRequest",
                    "arguments":["reloadProjects",{}, {"executionTarget":0,"expectsResult":true,"isAsync":false,"lowPriority":false}]
                }), cancel, None).await?;
                if response.get("success") == Some(&Value::Bool(false)) {
                    return Err(error("typescript-language-server rejected reloadProjects after a file create"));
                }
                Ok(())
            }
        })).await;
        for result in results { result?; }
        Ok(())
    }

    async fn reconcile_client_from_disk(&self, client: &Client, path: &Path, cancel: Option<CancellationToken>) -> io::Result<bool> {
        check_cancel(cancel.as_ref())?;
        let uri = file_uri(path);
        let key = uri_key(&uri);
        let open = client.state.lock().unwrap().documents.contains_key(&key);
        if open && self.disk_write_pending(&key) { return Ok(false); }
        let lock = client.document_lock(&uri);
        let _owner = tokio::select! { owner = lock.lock() => owner, _ = cancellation(cancel.as_ref()) => return Err(cancelled()) };
        check_cancel(cancel.as_ref())?;
        let previous = client.state.lock().unwrap().documents.get(&key).cloned();
        if previous.is_some() && self.disk_write_pending(&key) { return Ok(false); }
        let content = match tokio::fs::read(path).await {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        };
        check_cancel(cancel.as_ref())?;
        // The awaited read may have overlapped the beginning of a mutation.
        if self.disk_write_pending(&key) { return Ok(false); }
        if previous.as_ref().is_some_and(|document| document.synced_content == content) { return Ok(false); }
        let language = client.server.language_id.clone().unwrap_or_else(|| language_id(path));
        let version = previous.as_ref().map(|document| document.version + 1).unwrap_or(1);
        client.state.lock().unwrap().diagnostics.remove(&key);
        if previous.is_some() {
            client.notify("textDocument/didChange", json!({"textDocument":{"uri":uri,"version":version},"contentChanges":[{"text":content}]}), cancel).await?;
        } else {
            client.notify("textDocument/didOpen", json!({"textDocument":{"uri":uri,"languageId":language,"version":version,"text":content}}), cancel).await?;
        }
        client.state.lock().unwrap().documents.insert(key, Document { version, language, synced_content: content });
        Ok(previous.is_some())
    }

    /// Existing Host diagnostic/semantic queries use this owner and document
    /// lock. Returns document versions only for overlays actually changed.
    pub async fn reconcile_file_from_disk(&self, path: &Path, servers: &[ServerConfig], cancel: Option<CancellationToken>) -> io::Result<Versions> {
        let mut changed = Versions::new();
        for server in servers.iter().filter(|server| !server.custom) {
            if let Some(client) = self.get(server, cancel.clone(), true).await? {
                if self.reconcile_client_from_disk(&client, path, cancel.clone()).await? {
                    let version = client.state.lock().unwrap().documents.get(&uri_key(&file_uri(path))).map(|document| document.version);
                    if let Some(version) = version { changed.insert(server.name.clone(), version); }
                }
            }
        }
        Ok(changed)
    }

    pub fn servers_for_file(&self, path: &Path) -> Vec<ServerConfig> {
        config::servers_for_file(&self.config, path)
    }
    pub(super) async fn get(
        &self,
        server: &ServerConfig,
        cancel: Option<CancellationToken>,
        create_missing: bool,
    ) -> io::Result<Option<Arc<Client>>> {
        check_cancel(cancel.as_ref())?;
        if self.closing.load(Ordering::Acquire) {
            return Err(error("LSP pool is shutting down"));
        }
        if server.custom {
            return Ok(None);
        }
        let key = identity(server, &self.cwd);
        let slot = {
            let mut slots = self.slots.lock().await;
            match slots.get(&key) {
                Some(slot) => slot.clone(),
                None if create_missing => {
                    let slot = Arc::new(Mutex::new(Slot { client: None, failure: None }));
                    slots.insert(key, slot.clone());
                    slot
                }
                None => return Ok(None),
            }
        };
        let mut slot = tokio::select! { slot = slot.lock() => slot, _ = cancellation(cancel.as_ref()) => return Err(cancelled()), _ = self.creation_cancel.cancelled() => return Err(cancelled()) };
        if let Some(client) = &slot.client {
            if client.alive() {
                client.state.lock().unwrap().activity = Instant::now();
                return Ok(Some(client.clone()));
            }
        }
        slot.client = None;
        if !create_missing {
            return Ok(None);
        }
        if let Some((time, message)) = &slot.failure {
            if time.elapsed() < Duration::from_secs(180) {
                return Err(error(format!("LSP server failed to initialize recently: {message}")));
            }
        }
        let result = tokio::select! {result=Client::start(server.clone(), self.cwd.clone(), &self.agent_dir, self.shared, &self.lifetime, &self.creation_cancel, &self.owned_clients,cancel)=>result,_=self.creation_cancel.cancelled()=>Err(cancelled())};
        match result {
            Ok(client) => {
                if let Some(presence) = &client.presence {
                    self.presences.lock().unwrap().push(presence.clone());
                }
                slot.failure = None;
                slot.client = Some(client.clone());
                Ok(Some(client))
            }
            Err(error) => {
                if !matches!(error.kind(), io::ErrorKind::Interrupted | io::ErrorKind::TimedOut) {
                    slot.failure = Some((Instant::now(), error.to_string()));
                }
                Err(error)
            }
        }
    }
    pub async fn warmup(&self, cancel: Option<CancellationToken>) -> io::Result<()> {
        let results = join_all(self.config.servers.iter().filter(|server| !server.custom).map(|server| {
            let cancel = cancel.clone();
            async move {
                let bound = cancel.as_ref().map(CancellationToken::child_token).unwrap_or_default();
                let result = tokio::time::timeout(
                    Duration::from_millis(server.warmup_timeout_ms.unwrap_or(5000)),
                    self.get(server, Some(bound.clone()), true),
                )
                .await;
                if result.is_err() {
                    bound.cancel();
                }
                result
                    .map_err(|_| {
                        io::Error::new(io::ErrorKind::TimedOut, format!("LSP warmup {} timed out", server.name))
                    })?
                    .map(|_| ())
            }
        }))
        .await;
        check_cancel(cancel.as_ref())?;
        if let Some(error) = results.into_iter().find_map(Result::err) {
            return Err(error);
        }
        Ok(())
    }
    pub async fn shutdown(&self) -> io::Result<()> {
        self.closing.store(true, Ordering::Release);
        self.creation_cancel.cancel();
        let slots = self.slots.lock().await.values().cloned().collect::<Vec<_>>();
        for slot in slots {
            slot.lock().await.client.take();
        }
        let clients = std::mem::take(&mut *self.owned_clients.lock().unwrap());
        let results = join_all(clients.iter().map(|client| client.shutdown())).await;
        self.lifetime.cancel();
        let presences = std::mem::take(&mut *self.presences.lock().unwrap());
        join_all(presences.iter().map(|presence| presence.shutdown())).await;
        results.into_iter().collect::<io::Result<Vec<_>>>().map(|_| ())
    }
    pub async fn capture_diagnostic_versions(
        &self,
        servers: &[ServerConfig],
        cancel: Option<CancellationToken>,
    ) -> Versions {
        let pairs = join_all(servers.iter().filter(|server| !server.custom).map(|server| {
            let cancel = cancel.clone();
            async move {
                let client = self.get(server, cancel.clone(), true).await.ok()??;
                let version = client.state.lock().unwrap().diagnostic_version;
                Some((server.name.clone(), version))
            }
        }))
        .await;
        pairs.into_iter().flatten().collect()
    }
    pub async fn capture_open_versions(
        &self,
        path: &Path,
        servers: &[ServerConfig],
        cancel: Option<CancellationToken>,
    ) -> Versions {
        let key = uri_key(&file_uri(path));
        let pairs = join_all(servers.iter().filter(|server| !server.custom).map(|server| {
            let key = &key;
            let cancel = cancel.clone();
            async move {
                let client = self.get(server, cancel.clone(), true).await.ok()??;
                let version = client.state.lock().unwrap().documents.get(key).map(|doc| doc.version)?;
                Some((server.name.clone(), version))
            }
        }))
        .await;
        pairs.into_iter().flatten().collect()
    }
    pub async fn sync_content(
        &self,
        path: &Path,
        content: &str,
        servers: &[ServerConfig],
        cancel: Option<CancellationToken>,
        create_missing: bool,
    ) -> io::Result<()> {
        check_cancel(cancel.as_ref())?;
        let _results = join_all(servers.iter().filter(|server| !server.custom).map(|server| {
            let cancel = cancel.clone();
            async move {
                if let Some(client) = self.get(server, cancel.clone(), create_missing).await? {
                    client.sync(path, content, cancel.clone()).await?;
                }
                Ok::<_, io::Error>(())
            }
        }))
        .await;
        check_cancel(cancel.as_ref())
    }
    pub async fn saved(
        &self,
        path: &Path,
        servers: &[ServerConfig],
        cancel: Option<CancellationToken>,
        create_missing: bool,
    ) -> io::Result<()> {
        check_cancel(cancel.as_ref())?;
        let _results = join_all(servers.iter().filter(|server| !server.custom).map(|server| {
            let cancel = cancel.clone();
            async move {
                if let Some(client) = self.get(server, cancel.clone(), create_missing).await? {
                    client.saved(path, cancel.clone()).await?;
                }
                Ok::<_, io::Error>(())
            }
        }))
        .await;
        check_cancel(cancel.as_ref())
    }
    pub async fn format(
        &self,
        path: &Path,
        content: &str,
        servers: &[ServerConfig],
        cancel: Option<CancellationToken>,
    ) -> FormatResult {
        diagnostics::format(self, path, content, servers, cancel).await
    }
    pub async fn diagnostics(
        &self,
        path: &Path,
        servers: &[ServerConfig],
        min_versions: Option<&Versions>,
        expected_versions: Option<&Versions>,
        wait_ms: u64,
        cancel: Option<CancellationToken>,
    ) -> io::Result<Option<Value>> {
        let reconciled = if min_versions.is_none() && expected_versions.is_none() {
            Some(self.reconcile_file_from_disk(path, servers, cancel.clone()).await?)
        } else { None };
        let expected = expected_versions.or_else(|| reconciled.as_ref().filter(|versions| !versions.is_empty()));
        diagnostics::diagnostics(self, path, servers, min_versions, expected, wait_ms, cancel).await
    }
    pub async fn watched(
        &self,
        changes: &[ara_tools::edit_host::EditFileChange],
        cancel: Option<CancellationToken>,
    ) -> io::Result<()> {
        check_cancel(cancel.as_ref())?;
        let changes = changes.iter().filter_map(|change| { let path = absolute(&change.path); if !path.starts_with(&self.cwd) { return None; } Some(json!({"uri":file_uri(&path),"type":match change.change_type { ara_tools::edit_host::EditFileChangeType::Created => 1, ara_tools::edit_host::EditFileChangeType::Changed => 2, ara_tools::edit_host::EditFileChangeType::Deleted => 3 }})) }).collect::<Vec<_>>();
        if changes.is_empty() {
            return Ok(());
        }
        let slots = self.slots.lock().await.values().cloned().collect::<Vec<_>>();
        let mut clients = Vec::new();
        for slot in slots {
            if let Ok(slot) = slot.try_lock() {
                if let Some(client) = &slot.client {
                    if client.alive() {
                        clients.push(client.clone());
                    }
                }
            }
        }
        let _results = join_all(clients.into_iter().map(|client| {
            let changes = &changes;
            let cancel = &cancel;
            async move {
                {
                    let mut state = client.state.lock().unwrap();
                    for change in changes {
                        if let Some(uri) = change["uri"].as_str() {
                            state.diagnostics.remove(&uri_key(uri));
                        }
                    }
                }
                let bound = cancel.as_ref().map(CancellationToken::child_token).unwrap_or_default();
                let result = tokio::time::timeout(
                    Duration::from_secs(2),
                    client.notify("workspace/didChangeWatchedFiles", json!({"changes":changes}), Some(bound.clone())),
                )
                .await;
                if result.is_err() {
                    bound.cancel();
                }
                result
            }
        }))
        .await;
        check_cancel(cancel.as_ref())
    }
    pub(super) fn cwd(&self) -> &Path {
        &self.cwd
    }
}
fn identity(server: &ServerConfig, cwd: &Path) -> String {
    let value = json!([
        server.resolved_command,
        server.args,
        cwd,
        server.env,
        server.init_options,
        server.settings,
        server.language_id
    ]);
    let hash = ring::digest::digest(&ring::digest::SHA256, value.to_string().as_bytes());
    hash.as_ref().iter().map(|byte| format!("{byte:02x}")).collect()
}
fn client_capabilities() -> Value {
    json!({"textDocument":{"synchronization":{"didSave":true,"dynamicRegistration":false,"willSave":false,"willSaveWaitUntil":false},"hover":{"contentFormat":["markdown","plaintext"],"dynamicRegistration":false},"definition":{"dynamicRegistration":false,"linkSupport":true},"typeDefinition":{"dynamicRegistration":false,"linkSupport":true},"implementation":{"dynamicRegistration":false,"linkSupport":true},"references":{"dynamicRegistration":false},"documentSymbol":{"dynamicRegistration":false,"hierarchicalDocumentSymbolSupport":true,"symbolKind":{"valueSet":(1..=26).collect::<Vec<_>>()}},"rename":{"dynamicRegistration":false,"prepareSupport":true},"codeAction":{"dynamicRegistration":false,"codeActionLiteralSupport":{"codeActionKind":{"valueSet":["quickfix","refactor","refactor.extract","refactor.inline","refactor.rewrite","source","source.organizeImports","source.fixAll"]}},"resolveSupport":{"properties":["edit"]}},"formatting":{"dynamicRegistration":false},"rangeFormatting":{"dynamicRegistration":false},"publishDiagnostics":{"relatedInformation":true,"versionSupport":true,"tagSupport":{"valueSet":[1,2]},"codeDescriptionSupport":true,"dataSupport":true},"diagnostic":{"dynamicRegistration":true}},"window":{"workDoneProgress":true},"workspace":{"applyEdit":false,"configuration":true,"workspaceFolders":true,"symbol":{"dynamicRegistration":false,"symbolKind":{"valueSet":(1..=26).collect::<Vec<_>>() }},"fileOperations":{"dynamicRegistration":false,"willCreate":false,"didCreate":false,"willRename":true,"didRename":true,"willDelete":false,"didDelete":false}}})
}
fn language_id(path: &Path) -> String {
    let base = path.file_name().and_then(|v| v.to_str()).unwrap_or_default().to_lowercase();
    if base == "dockerfile" || base.starts_with("dockerfile.") || base == "containerfile" {
        return "dockerfile".into();
    }
    if base == ".emacs" {
        return "emacs-lisp".into();
    }
    if matches!(base.as_str(), "makefile" | "gnumakefile") {
        return "makefile".into();
    }
    if base == "justfile" {
        return "just".into();
    }
    if base == "cmakelists.txt" {
        return "cmake".into();
    }
    let ext = path.extension().and_then(|v| v.to_str()).unwrap_or_default().to_lowercase();
    match ext.as_str() {
        "ts" | "cts" | "mts" => "typescript",
        "tsx" => "typescriptreact",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "rs" => "rust",
        "go" => "go",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hh" | "hpp" | "hxx" | "cu" | "ino" => "cpp",
        "py" | "pyi" => "python",
        "rb" | "rbw" | "gemspec" => "ruby",
        "sh" | "bash" | "zsh" | "ksh" | "bats" | "tmux" | "cgi" | "fcgi" | "command" | "tool" => "shellscript",
        "kt" | "ktm" | "kts" => "kotlin",
        "scala" | "sc" | "sbt" => "scala",
        "clj" | "cljc" | "cljs" | "edn" => "clojure",
        "el" => "emacs-lisp",
        "md" | "markdown" | "mdx" | "mdc" | "mkd" | "mdown" => "markdown",
        "rst" => "restructuredtext",
        "adoc" => "asciidoc",
        "tex" => "latex",
        "tf" | "tfvars" => "terraform",
        "gql" | "graphql" => "graphql",
        "proto" => "protobuf",
        "pl" | "pm" | "perl" => "perl",
        "yaml" | "yml" => "yaml",
        "xml" | "xsl" | "xslt" | "svg" | "plist" => "xml",
        "ex" | "exs" => "elixir",
        "erl" | "hrl" => "erlang",
        "hs" => "haskell",
        "ml" | "mli" => "ocaml",
        "ps1" | "psm1" => "powershell",
        "bat" | "cmd" => "bat",
        "tla" | "tlaplus" => "tlaplus",
        "jl" => "julia",
        "lua" => "lua",
        "java" => "java",
        "cs" => "csharp",
        "swift" => "swift",
        "r" => "r",
        "json" => "json",
        "jsonc" => "jsonc",
        "toml" => "toml",
        "cmake" => "cmake",
        "css" => "css",
        "scss" => "scss",
        "sass" => "sass",
        "less" => "less",
        "html" => "html",
        "vue" => "vue",
        "svelte" => "svelte",
        "astro" => "astro",
        "sql" => "sql",
        "zig" => "zig",
        "fish" => "fish",
        "php" => "php",
        "groovy" => "groovy",
        "nix" => "nix",
        "dart" => "dart",
        "elm" => "elm",
        "v" => "v",
        "nim" => "nim",
        "cr" => "crystal",
        "d" => "d",
        "pas" | "pp" => "pascal",
        "lisp" | "lsp" => "lisp",
        "rkt" => "racket",
        "scm" => "scheme",
        _ => "plaintext",
    }
    .into()
}


#[cfg(test)]
mod upgrade_barrier_tests {
    use super::*;
    #[tokio::test]
    async fn pending_disk_barrier_balances_nested_writes_and_admitted_leases(){
        let dir=tempfile::tempdir().unwrap();let pool=ClientPool::new(dir.path().to_owned(),dir.path().join("agent"),LspConfig {servers:vec![],idle_timeout_ms:None,source_snapshot:vec![],unresolved_plugin_sources:false},false);
        let path=dir.path().join("overlay.ts");let key=uri_key(&file_uri(&path));
        let first=pool.begin_pending_disk_write(&path);let admitted=first.clone();let second=pool.begin_pending_disk_write(&path);
        drop(first);drop(second);assert!(pool.disk_write_pending(&key),"admitted disk effect retains barrier after caller drops");
        drop(admitted);assert!(!pool.disk_write_pending(&key));pool.shutdown().await.unwrap();
    }
}
