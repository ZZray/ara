//! Fixed OMP 596f2da lsp/writethrough.ts, hosted production supplier.
//! Copyright (c) 2025-2026 Can Bölük; (c) 2026 Stencil Labs, Inc.
//! MIT; see THIRD_PARTY_NOTICES.md. Session sinks travel with requests.

use super::{client::ClientPool, config::ServerConfig, types::Versions};
use ara_tools::edit_file_fallback::{EditFileMutationFallbackRegistry, is_permission_denied};
use ara_tools::edit_host::{
    EditDeferredDiagnosticsFactory, EditDeferredDiagnosticsHandle, EditDiagnosticsTransform, EditFileChange,
    EditFileChangeType, EditLspOptions, EditLspWriteRequest, EditLspWritethrough, EditLspWritethroughResult,
};
use async_trait::async_trait;
use futures::future::{BoxFuture, FutureExt, Shared};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

type CommitResult = Result<(), Arc<io::Error>>;
type CommitFuture = Shared<BoxFuture<'static, CommitResult>>;

fn copy_error(error: &io::Error) -> io::Error {
    error
        .raw_os_error()
        .map(io::Error::from_raw_os_error)
        .unwrap_or_else(|| io::Error::new(error.kind(), error.to_string()))
}
fn cancelled() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "Aborted: Cancelled")
}
fn check_cancel(cancel: Option<&CancellationToken>) -> io::Result<()> {
    if cancel.is_some_and(CancellationToken::is_cancelled) { Err(cancelled()) } else { Ok(()) }
}
async fn wait_cancel(cancel: Option<CancellationToken>) {
    if let Some(cancel) = cancel {
        cancel.cancelled().await;
    } else {
        std::future::pending::<()>().await;
    }
}

/// Serialize distinct committed contents, retaining every admitted future until
/// settlement. A failed/unknown write is never replayed; a formatter's different
/// bytes are a new commit only after the preceding commit succeeded.
struct CommitState {
    disk_content: Option<String>,
    pending: Option<(String, CommitFuture)>,
    failure: Option<Arc<io::Error>>,
}
struct MemoizedCommit {
    path: PathBuf,
    content: Mutex<String>,
    fallback: Option<Arc<EditFileMutationFallbackRegistry>>,
    session: Option<String>,
    state: AsyncMutex<CommitState>,
    disk_barrier: Mutex<Option<super::client::PendingDiskWrite>>,
}
impl MemoizedCommit {
    fn new(entry: &PendingWrite, already_written: bool) -> Arc<Self> {
        Arc::new(Self {
            path: entry.path.clone(),
            content: Mutex::new(entry.content.clone()),
            fallback: entry.fallback.clone(),
            session: entry.session.clone(),
            disk_barrier: Mutex::new(None),
            state: AsyncMutex::new(CommitState {
                disk_content: already_written.then(|| entry.content.clone()),
                pending: None,
                failure: None,
            }),
        })
    }
    fn set_barrier(&self, barrier: Option<super::client::PendingDiskWrite>) {
        *self.disk_barrier.lock().unwrap_or_else(|error| error.into_inner()) = barrier;
    }
    fn set_content(&self, content: String) {
        *self.content.lock().unwrap_or_else(|e| e.into_inner()) = content;
    }
    fn content(&self) -> String {
        self.content.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    async fn settle_admitted(&self) -> io::Result<()> {
        let mut state = self.state.lock().await;
        if let Some((content, future)) = state.pending.clone() {
            match future.await {
                Ok(()) => { state.disk_content = Some(content); state.pending = None; }
                Err(error) => { state.failure = Some(error.clone()); return Err(copy_error(&error)); }
            }
        }
        if let Some(error) = &state.failure { return Err(copy_error(error)); }
        Ok(())
    }
    async fn commit(&self) -> io::Result<()> {
        self.settle_admitted().await?;
        let content = self.content();
        let mut state = self.state.lock().await;
        if let Some(error) = &state.failure { return Err(copy_error(error)); }
        if state.disk_content.as_deref() == Some(content.as_str()) { return Ok(()); }
        // A concurrent observer can have admitted a commit while this caller
        // reacquired the state. Settle that exact future before considering a
        // different content; do not replace a pending external effect.
        if let Some((settled_content, future)) = state.pending.clone() {
            match future.await {
                Ok(()) => { state.disk_content = Some(settled_content); state.pending = None; }
                Err(error) => { state.failure = Some(error.clone()); return Err(copy_error(&error)); }
            }
            if state.disk_content.as_deref() == Some(content.as_str()) { return Ok(()); }
        }
        let path = self.path.clone();
        let fallback = self.fallback.clone();
        let session = self.session.clone();
        let supplied = content.clone();
        let barrier = self.disk_barrier.lock().unwrap_or_else(|error| error.into_inner()).clone();
        let operation = tokio::spawn(async move {
            let _barrier = barrier;
            write_bytes(&path, &supplied, fallback.as_ref(), session.as_deref()).await.map_err(Arc::new)
        });
        let future = async move {
            operation.await.unwrap_or_else(|error| {
                Err(Arc::new(io::Error::other(format!("admitted LSP write settlement failed: {error}"))))
            })
        }.boxed().shared();
        state.pending = Some((content.clone(), future.clone()));
        match future.await {
            Ok(()) => { state.disk_content = Some(content); state.pending = None; Ok(()) }
            Err(error) => { state.failure = Some(error.clone()); Err(copy_error(&error)) }
        }
    }
}
async fn write_bytes(
    path: &Path,
    content: &str,
    fallback: Option<&Arc<EditFileMutationFallbackRegistry>>,
    session: Option<&str>,
) -> io::Result<()> {
    if let Some(fallback) = fallback {
        if let Some(parent) = path.parent() {
            fallback.mkdir_allowing_fallback(parent)?;
        }
        fallback.write(path, content, session).await.map_err(|error| copy_error(error.original()))
    } else {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(path, content.as_bytes()).await
    }
}

#[derive(Clone)]
struct PendingWrite {
    path: PathBuf,
    content: String,
    change_type: EditFileChangeType,
    fallback: Option<Arc<EditFileMutationFallbackRegistry>>,
    session: Option<String>,
}
#[derive(Clone)]
struct Options {
    values: EditLspOptions,
    transform: Option<EditDiagnosticsTransform>,
}
struct BatchState {
    entries: Vec<PendingWrite>,
    options: Options,
}
struct PendingLate {
    task: tokio::task::JoinHandle<io::Result<Option<Value>>>,
    deferred: EditDeferredDiagnosticsHandle,
    cancel: CancellationToken,
}
struct PipelineResult {
    value: Option<Value>,
    late: Option<PendingLate>,
    final_content: String,
}
impl PipelineResult {
    fn inline(value: Option<Value>) -> Self {
        Self { value, late: None, final_content: String::new() }
    }
}

/// Process supplier: shared LSP transport and batches; never a Session ledger.
pub struct HostLspWritethrough {
    pool: Arc<ClientPool>,
    batches: Mutex<HashMap<String, BatchState>>,
    lifetime: CancellationToken,
    late_tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}
impl HostLspWritethrough {
    pub fn new(pool: Arc<ClientPool>) -> Arc<Self> {
        Arc::new(Self {
            pool,
            batches: Mutex::new(HashMap::new()),
            lifetime: CancellationToken::new(),
            late_tasks: Mutex::new(Vec::new()),
        })
    }
    fn take_batch(&self, id: &str) -> Option<BatchState> {
        self.batches.lock().unwrap_or_else(|e| e.into_inner()).remove(id)
    }
    fn record_batch(&self, id: &str, entry: PendingWrite, options: Options) {
        let mut batches = self.batches.lock().unwrap_or_else(|e| e.into_inner());
        let state = batches
            .entry(id.to_owned())
            .or_insert_with(|| BatchState { entries: Vec::new(), options: options.clone() });
        state.options.values.enable_format |= options.values.enable_format;
        state.options.values.enable_diagnostics |= options.values.enable_diagnostics;
        if state.options.transform.is_none() {
            state.options.transform = options.transform;
        }
        if let Some(index) = state.entries.iter().position(|old| old.path == entry.path) {
            state.entries[index] = entry;
        } else {
            state.entries.push(entry);
        }
    }
    fn handoff_late(&self, pending: PendingLate) {
        let lifetime = self.lifetime.clone();
        let task = tokio::spawn(async move {
            let PendingLate { mut task, deferred, cancel } = pending;
            let signal = deferred.signal();
            let result = tokio::select! {
                result = &mut task => result.ok().and_then(Result::ok).flatten(),
                _ = signal.cancelled() => { cancel.cancel(); let _ = task.await; None },
                _ = lifetime.cancelled() => { cancel.cancel(); let _ = task.await; None },
                _ = tokio::time::sleep(Duration::from_secs(25)) => { cancel.cancel(); let _ = task.await; None },
            };
            if !signal.is_cancelled() && !lifetime.is_cancelled() {
                if let Some(result) = result {
                    if let Err(error) = deferred.on_deferred_diagnostics(result) {
                        eprintln!("ara: late LSP diagnostics queue failed: {error}");
                    }
                }
            }
        });
        let mut tasks = self.late_tasks.lock().unwrap_or_else(|error| error.into_inner());
        tasks.retain(|task| !task.is_finished());
        tasks.push(task);
    }
    pub async fn shutdown(&self) {
        self.lifetime.cancel();
        let tasks = std::mem::take(&mut *self.late_tasks.lock().unwrap_or_else(|error| error.into_inner()));
        for task in tasks {
            let _ = task.await;
        }
    }
    fn finalize(&self, deferred: Option<EditDeferredDiagnosticsHandle>, result: PipelineResult) -> Option<Value> {
        if let Some(deferred) = deferred {
            deferred.finalize(result.value.as_ref());
        }
        // Finalize precedes callback publication, including a fast completion
        // immediately after the inline deadline. No callback enters a global slot.
        if let Some(pending) = result.late {
            self.handoff_late(pending);
        }
        result.value
    }
    async fn flush_state(
        &self,
        state: BatchState,
        cancel: Option<CancellationToken>,
        deferred: Option<EditDeferredDiagnosticsFactory>,
        requested: Option<&Path>,
    ) -> io::Result<EditLspWritethroughResult> {
        let mut results = Vec::new();
        let mut final_content = Some(requested.and_then(|requested| state.entries.iter().find(|entry| entry.path.as_path() == requested).map(|entry| entry.content.clone())).unwrap_or_default());
        for mut entry in state.entries {
            check_cancel(cancel.as_ref())?;
            let bundle = deferred.as_ref().and_then(|factory| factory(&entry.path));
            match tokio::fs::read(&entry.path).await {
                Ok(bytes) => entry.content = String::from_utf8_lossy(&bytes).into_owned(),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    if let Some(bundle) = bundle {
                        bundle.finalize(None);
                    }
                    continue;
                }
                Err(error) if is_permission_denied(&error) => {}
                Err(error) => return Err(error),
            }
            let requested_entry = requested == Some(entry.path.as_path());
            let result = self.run(entry, state.options.clone(), cancel.clone(), bundle.clone(), true).await?;
            if requested_entry { final_content = Some(result.final_content.clone()); }
            results.push(self.finalize(bundle, result));
        }
        Ok(EditLspWritethroughResult { diagnostics: merge_diagnostics(results, state.options.values.enable_diagnostics), final_content })
    }
    async fn fetch(
        &self,
        entry: &PendingWrite,
        servers: &[ServerConfig],
        min: Option<Versions>,
        expected: Option<Versions>,
        options: &Options,
        cancel: Option<CancellationToken>,
        deferred: Option<EditDeferredDiagnosticsHandle>,
    ) -> io::Result<PipelineResult> {
        let Some(deferred) = deferred else {
            let raw =
                self.pool.diagnostics(&entry.path, servers, min.as_ref(), expected.as_ref(), 3_000, cancel).await?;
            return Ok(PipelineResult::inline(apply_transform(&entry.path, raw, options.transform.as_ref())?));
        };
        let signal = deferred.signal();
        let pool = self.pool.clone();
        let path = entry.path.clone();
        let servers = servers.to_vec();
        let fetch_cancel = CancellationToken::new();
        let supplied_cancel = fetch_cancel.clone();
        let mut task = tokio::spawn(async move {
            pool.diagnostics(&path, &servers, min.as_ref(), expected.as_ref(), 12_000, Some(supplied_cancel)).await
        });
        tokio::select! {
            result = &mut task => {
                let result = result.map_err(|e| io::Error::other(format!("LSP diagnostics task failed: {e}")))??;
                Ok(PipelineResult::inline(apply_transform(&entry.path, result, options.transform.as_ref())?))
            },
            _ = signal.cancelled() => { fetch_cancel.cancel(); let _ = task.await; Ok(PipelineResult::inline(None)) },
            _ = wait_cancel(cancel) => { fetch_cancel.cancel(); let _ = task.await; Err(cancelled()) },
            _ = tokio::time::sleep(Duration::from_millis(500)) => {
                Ok(PipelineResult { value: None, late: Some(PendingLate { task, deferred, cancel: fetch_cancel }), final_content: String::new() })
            },
        }
    }
    async fn run(
        &self,
        entry: PendingWrite,
        options: Options,
        caller: Option<CancellationToken>,
        deferred: Option<EditDeferredDiagnosticsHandle>,
        already_written: bool,
    ) -> io::Result<PipelineResult> {
        check_cancel(caller.as_ref())?;
        let write = MemoizedCommit::new(&entry, already_written);
        let notified = Arc::new(Mutex::new(false));
        if !options.values.enable_format && !options.values.enable_diagnostics {
            write.commit().await?;
            self.notify_commit(&entry, &notified, caller.clone(), caller.as_ref()).await?;
            return Ok(PipelineResult { value: None, late: None, final_content: write.content() });
        }
        let servers = self.pool.servers_for_file(&entry.path);
        if servers.is_empty() {
            write.commit().await?;
            self.notify_commit(&entry, &notified, caller.clone(), caller.as_ref()).await?;
            return Ok(PipelineResult { value: None, late: None, final_content: write.content() });
        }
        let lsp = servers.iter().filter(|server| !server.custom).cloned().collect::<Vec<_>>();
        let custom = servers.iter().filter(|server| server.custom).cloned().collect::<Vec<_>>();
        let custom_formatter = options.values.enable_format && !custom.is_empty();
        let operation_cancel = CancellationToken::new();
        let mut min = None;
        if options.values.enable_diagnostics && !custom_formatter {
            let capture_cancel = CancellationToken::new();
            let capture = self.pool.capture_diagnostic_versions(&servers, Some(capture_cancel.clone()));
            tokio::pin!(capture);
            min = Some(tokio::select! {
                versions = &mut capture => versions,
                _ = tokio::time::sleep(Duration::from_secs(5)) => { capture_cancel.cancel(); capture.await },
                _ = wait_cancel(caller.clone()) => { capture_cancel.cancel(); let _ = capture.await; return Err(cancelled()); },
            });
        }
        let mut expected = None;
        let mut formatter = None;
        let mut external_write_unknown = false;
        // Hold the barrier until all admitted disk work has settled. It begins
        // before the first overlay sync and also covers created-file didOpen.
        let disk_barrier = self.pool.begin_pending_disk_write(&entry.path);
        write.set_barrier(Some(disk_barrier.clone()));
        let mut operation = Box::pin(async {
            if custom_formatter {
                // Custom clients inspect disk. The original pre-write is its
                // own admitted commit; final formatting uses the same primitive.
                write.commit().await?;
                let format = self.pool.format(&entry.path, &entry.content, &custom, Some(operation_cancel.clone()));
                let capture = async {
                    if options.values.enable_diagnostics {
                        Some(self.pool.capture_diagnostic_versions(&servers, Some(operation_cancel.clone())).await)
                    } else {
                        None
                    }
                };
                let (formatted, captured) = tokio::join!(format, capture);
                external_write_unknown = formatted.external_write_unknown;
                if external_write_unknown {
                    return Err(io::Error::other("formatter persisted outcome unknown; re-read before retrying"));
                }
                check_cancel(Some(&operation_cancel))?;
                min = captured;
                formatter = Some(format_state(&entry.content, &formatted));
                write.set_content(formatted.content);
                write.commit().await?;
                self.notify_commit(&entry, &notified, Some(operation_cancel.clone()), caller.as_ref()).await?;
                if entry.change_type == EditFileChangeType::Created {
                    self.pool.reload_typescript_after_create(&lsp, Some(operation_cancel.clone()), options.values.enable_diagnostics).await?;
                }
                self.pool
                    .sync_content(
                        &entry.path,
                        &write.content(),
                        &lsp,
                        Some(operation_cancel.clone()),
                        options.values.enable_diagnostics,
                    )
                    .await?;
            } else {
                if entry.change_type == EditFileChangeType::Created {
                    write.commit().await?;
                    self.notify_commit(&entry, &notified, Some(operation_cancel.clone()), caller.as_ref()).await?;
                    self.pool.reload_typescript_after_create(&lsp, Some(operation_cancel.clone()), true).await?;
                }
                self.pool.sync_content(&entry.path, &entry.content, &lsp, Some(operation_cancel.clone()), true).await?;
                if options.values.enable_format {
                    let formatted =
                        self.pool.format(&entry.path, &entry.content, &lsp, Some(operation_cancel.clone())).await;
                    check_cancel(Some(&operation_cancel))?;
                    formatter = Some(format_state(&entry.content, &formatted));
                    let changed = formatted.content != entry.content;
                    write.set_content(formatted.content);
                    if changed {
                        self.pool
                            .sync_content(&entry.path, &write.content(), &lsp, Some(operation_cancel.clone()), true)
                            .await?;
                    }
                }
                check_cancel(Some(&operation_cancel))?;
                write.commit().await?;
                self.notify_commit(&entry, &notified, Some(operation_cancel.clone()), caller.as_ref()).await?;
            }
            if options.values.enable_diagnostics {
                expected =
                    Some(self.pool.capture_open_versions(&entry.path, &lsp, Some(operation_cancel.clone())).await);
            }
            self.pool
                .saved(
                    &entry.path,
                    &lsp,
                    Some(operation_cancel.clone()),
                    !custom_formatter || options.values.enable_diagnostics,
                )
                .await?;
            Ok::<(), io::Error>(())
        });
        let (settled, timed_out, caller_cancelled) = tokio::select! {
            settled = &mut operation => (settled, false, false),
            _ = tokio::time::sleep(Duration::from_secs(5)) => {
                operation_cancel.cancel();
                ((&mut operation).await, true, false)
            },
            _ = wait_cancel(caller.clone()) => {
                operation_cancel.cancel();
                ((&mut operation).await, false, true)
            },
        };
        // Releases the operation's mutable captures before consulting versions.
        drop(operation);
        if external_write_unknown {
            return Err(io::Error::other("formatter persisted outcome unknown; re-read before retrying"));
        }
        write.settle_admitted().await?;
        if caller_cancelled || caller.as_ref().is_some_and(CancellationToken::is_cancelled) {
            // Settle an already admitted write, but do not admit a new one after
            // caller cancellation. The typed consumer marks unknown outcomes.
            let _ = write.settle_admitted().await;
            return Err(cancelled());
        }
        if settled.is_err() || timed_out {
            // Same memoized future whether a write was admitted or not. A
            // failed fallback stays a failure; no unknown write is repeated.
            write.commit().await?;
            self.notify_commit(&entry, &notified, caller.clone(), caller.as_ref()).await?;
        }
        if timed_out {
            let late = if options.values.enable_diagnostics {
                deferred.filter(|deferred| !deferred.signal().is_cancelled()).map(|deferred| {
                    let pool = self.pool.clone();
                    let path = entry.path.clone();
                    let servers = servers.clone();
                    let cancel = CancellationToken::new();
                    let supplied = cancel.clone();
                    let task = tokio::spawn(async move {
                        pool.diagnostics(&path, &servers, min.as_ref(), expected.as_ref(), 12_000, Some(supplied)).await
                    });
                    PendingLate { task, deferred, cancel }
                })
            } else {
                None
            };
            write.set_barrier(None);
            drop(disk_barrier);
            return Ok(PipelineResult { value: None, late, final_content: write.content() });
        }
        write.set_barrier(None);
        drop(disk_barrier);
        let mut result = if settled.is_ok() && options.values.enable_diagnostics {
            self.fetch(&entry, &servers, min, expected, &options, caller, deferred).await?
        } else {
            PipelineResult::inline(None)
        };
        if let Some(formatter) = formatter {
            let value = result.value.get_or_insert_with(|| {
                json!({
                    "server":servers.iter().map(|server| server.name.as_str()).collect::<Vec<_>>().join(", "),
                    "messages":[],"summary":"OK","errored":false
                })
            });
            value["formatter"] = json!(formatter);
        }
        result.final_content = write.content();
        Ok(result)
    }
    async fn notify_commit(
        &self,
        entry: &PendingWrite,
        notified: &Mutex<bool>,
        signal: Option<CancellationToken>,
        caller: Option<&CancellationToken>,
    ) -> io::Result<()> {
        {
            let mut notified = notified.lock().unwrap_or_else(|e| e.into_inner());
            if *notified {
                return Ok(());
            }
            *notified = true;
        }
        let changes = [EditFileChange { path: entry.path.clone(), change_type: entry.change_type }];
        match self.pool.watched(&changes, signal.clone()).await {
            Err(_)
                if signal.as_ref().is_some_and(CancellationToken::is_cancelled)
                    && !caller.is_some_and(CancellationToken::is_cancelled) =>
            {
                *notified.lock().unwrap_or_else(|e| e.into_inner()) = false;
                Ok(())
            }
            result => result,
        }
    }
}
impl Drop for HostLspWritethrough {
    fn drop(&mut self) {
        self.lifetime.cancel();
    }
}

#[async_trait]
impl EditLspWritethrough for HostLspWritethrough {
    async fn write_through(&self, request: EditLspWriteRequest<'_>) -> io::Result<Option<Value>> {
        Ok(self.write_through_result(request).await?.diagnostics)
    }
    async fn write_through_result(&self, request: EditLspWriteRequest<'_>) -> io::Result<EditLspWritethroughResult> {
        check_cancel(request.cancel.as_ref())?;
        let entry = PendingWrite {
            path: request.absolute_path.to_owned(),
            content: request.content.to_owned(),
            change_type: if request.absolute_path.exists() {
                EditFileChangeType::Changed
            } else {
                EditFileChangeType::Created
            },
            fallback: request.file_fallbacks.clone(),
            session: request.mutation_session_id.clone(),
        };
        let options = Options { values: request.options, transform: request.transform_diagnostics.clone() };
        if let Some(batch) = request.batch {
            let write = MemoizedCommit::new(&entry, false);
            if let Err(error) = write.commit().await {
                if batch.flush {
                    if let Some(state) = self.take_batch(&batch.id) {
                        if let Err(flush) = self.flush_state(state, request.cancel, request.get_deferred, None).await {
                            eprintln!("ara: failed to flush pending LSP batch after final write failure: {flush}");
                        }
                    }
                }
                return Err(error);
            }
            let requested = entry.path.clone();
            let committed = entry.content.clone();
            self.record_batch(&batch.id, entry, options);
            if !batch.flush {
                return Ok(EditLspWritethroughResult { diagnostics: None, final_content: Some(committed) });
            }
            let Some(state) = self.take_batch(&batch.id) else {
                return Ok(EditLspWritethroughResult { diagnostics: None, final_content: Some(committed) });
            };
            self.flush_state(state, request.cancel, request.get_deferred, Some(&requested)).await
        } else {
            let deferred = request.get_deferred.as_ref().and_then(|factory| factory(&entry.path));
            let result = self.run(entry, options, request.cancel, deferred.clone(), false).await?;
            let final_content = Some(result.final_content.clone());
            Ok(EditLspWritethroughResult { diagnostics: self.finalize(deferred, result), final_content })
        }
    }
    async fn notify_watched_files(
        &self,
        _cwd: &Path,
        changes: &[EditFileChange],
        cancel: Option<CancellationToken>,
    ) -> io::Result<()> {
        self.pool.watched(changes, cancel).await
    }
    async fn flush_batch(&self, id: &str, _cwd: &Path, cancel: Option<CancellationToken>) -> io::Result<Option<Value>> {
        let Some(state) = self.take_batch(id) else {
            return Ok(None);
        };
        Ok(self.flush_state(state, cancel, None, None).await?.diagnostics)
    }
}
fn apply_transform(
    path: &Path,
    value: Option<Value>,
    transform: Option<&EditDiagnosticsTransform>,
) -> io::Result<Option<Value>> {
    match (value, transform) {
        (Some(value), Some(transform)) => transform(path, value).map(Some),
        (value, _) => Ok(value),
    }
}
fn format_state(original: &str, formatted: &super::types::FormatResult) -> &'static str {
    if formatted.failed {
        "failed"
    } else if formatted.unsupported {
        "unsupported"
    } else if formatted.content != original {
        "formatted"
    } else {
        "unchanged"
    }
}
fn merge_diagnostics(results: Vec<Option<Value>>, diagnostics: bool) -> Option<Value> {
    let mut messages = Vec::new();
    let mut servers = Vec::new();
    let mut formatter = None;
    let mut has_result = false;
    for result in results.into_iter().flatten() {
        has_result = true;
        if let Some(server) = result["server"].as_str() {
            for name in server.split(',').map(str::trim).filter(|name| !name.is_empty()) {
                if !servers.iter().any(|old| old == name) {
                    servers.push(name.to_owned());
                }
            }
        }
        if let Some(values) = result["messages"].as_array() {
            messages.extend(values.iter().filter_map(Value::as_str).map(str::to_owned));
        }
        if let Some(next) = result["formatter"].as_str() {
            let rank = |value| match value {
                "failed" => 4,
                "formatted" => 3,
                "unsupported" => 2,
                "unchanged" => 1,
                _ => 0,
            };
            if formatter.is_none_or(|old| rank(next) > rank(old)) {
                formatter = Some(match next {
                    "failed" => "failed",
                    "formatted" => "formatted",
                    "unsupported" => "unsupported",
                    _ => "unchanged",
                });
            }
        }
    }
    if !has_result {
        return None;
    }
    let kinds = ["error", "warning", "info", "hint"];
    static SEVERITY: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"(?i)\[(error|warning|info|hint)\]").unwrap());
    let mut counts = [0usize; 4];
    for message in &messages {
        if let Some(severity) = SEVERITY.captures(message) {
            if let Some(index) = kinds.iter().position(|kind| *kind == severity[1].to_ascii_lowercase()) {
                counts[index] += 1;
            }
        }
    }
    let summary = if messages.is_empty() {
        if diagnostics { "no issues".into() } else { "OK".into() }
    } else {
        kinds
            .into_iter()
            .zip(counts)
            .filter(|(_, count)| *count > 0)
            .map(|(kind, count)| format!("{count} {kind}(s)"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    messages.truncate(50);
    let mut value = json!({"messages":messages,"summary":summary,"errored":counts[0]>0});
    if !servers.is_empty() {
        value["server"] = json!(servers.join(", "));
    }
    if let Some(formatter) = formatter {
        value["formatter"] = json!(formatter);
    }
    Some(value)
}


#[cfg(test)]
mod upgrade_commit_tests {
    use super::*;
    use ara_tools::edit_file_fallback::{EditMutationFileSystem,EditMutationFileType,OsEditMutationFileSystem};
    use std::sync::atomic::{AtomicUsize,Ordering};
    struct CountingFs {calls:AtomicUsize,fail_after_write:bool}
    impl EditMutationFileSystem for CountingFs {
        fn write(&self,p:&Path,c:&str)->io::Result<()>{self.calls.fetch_add(1,Ordering::SeqCst);std::fs::write(p,c)?;if self.fail_after_write {Err(io::Error::other("unknown effect after real write"))}else{Ok(())}}
        fn unlink(&self,p:&Path)->io::Result<()>{OsEditMutationFileSystem.unlink(p)}
        fn mkdir(&self,p:&Path)->io::Result<()>{OsEditMutationFileSystem.mkdir(p)}
        fn realpath(&self,p:&Path)->io::Result<PathBuf>{OsEditMutationFileSystem.realpath(p)}
        fn lstat(&self,p:&Path)->io::Result<EditMutationFileType>{OsEditMutationFileSystem.lstat(p)}
    }
    #[tokio::test]
    async fn committed_different_bytes_once_and_failed_unknown_never_replayed(){
        let dir=tempfile::tempdir().unwrap();
        for fail in [false,true] {
            let io=Arc::new(CountingFs {calls:AtomicUsize::new(0),fail_after_write:fail});
            let fallback=EditFileMutationFallbackRegistry::new(io.clone(),Arc::new(|_|{}));
            let entry=PendingWrite {path:dir.path().join(if fail {"failed"}else{"ok"}),content:"original".into(),change_type:EditFileChangeType::Created,fallback:Some(fallback),session:Some("original-session".into())};
            let commit=MemoizedCommit::new(&entry,false);
            assert_eq!(commit.commit().await.is_err(),fail);assert_eq!(io.calls.load(Ordering::SeqCst),1);
            assert_eq!(commit.commit().await.is_err(),fail);assert_eq!(io.calls.load(Ordering::SeqCst),1);
            commit.set_content("formatted".into());assert_eq!(commit.commit().await.is_err(),fail);
            assert_eq!(io.calls.load(Ordering::SeqCst),if fail {1}else{2});
            assert_eq!(std::fs::read_to_string(&entry.path).unwrap(),if fail {"original"}else{"formatted"});
            assert_eq!(commit.commit().await.is_err(),fail);assert_eq!(io.calls.load(Ordering::SeqCst),if fail {1}else{2});
        }
    }
}
