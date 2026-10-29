//! Per-call EditSession pool and coalesced preview pump, fixed OMP
//! edit/index.ts:401-474 and pi-natives/src/edit.rs (MIT).
use super::*;
use crate::edit_urls::EditUrlResolver;
use ara_agent::tool::ActualToolArgStreamArguments as ValueHandle;
use ara_agent::{SessionValues, ToolArgStream, ToolArgStreamInit};
use ara_rpc::value_heap::StringifyMode;
use std::collections::{HashSet, VecDeque};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::Notify;

#[derive(Default)]
pub(super) struct SessionPool {
    entries: Mutex<VecDeque<(String, Arc<Shared>)>>,
}

enum ArgOp {
    Push(String),
    Finish,
}
struct Shared {
    session: Mutex<Session>,
    resolver: Arc<EditUrlResolver>,
    queue: Mutex<Vec<ArgOp>>,
    streamed_args: Mutex<Option<String>>,
    closed: AtomicBool,
    applying: AtomicBool,
    wake: Notify,
}
impl Shared {
    fn enqueue(&self, op: ArgOp) {
        let mut queue = self.queue.lock().unwrap_or_else(|error| error.into_inner());
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        queue.push(op);
        self.wake.notify_one();
    }
    fn drain(&self, session: &mut Session) {
        let pending =
            std::mem::take(&mut *self.queue.lock().unwrap_or_else(|error| error.into_inner()));
        for op in pending {
            match op {
                ArgOp::Push(delta) => session.push(&delta),
                ArgOp::Finish => session.finish(),
            }
        }
    }
    fn release(&self, session: &mut Session) {
        self.queue
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
        *session = Session::new(session.config().clone(), session.store().clone());
    }
    fn stop_previewing(&self, session: &mut Session) -> bool {
        if !self.closed.load(Ordering::Acquire) {
            return false;
        }
        if !self.applying.load(Ordering::Acquire) {
            self.release(session);
        }
        true
    }
    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.streamed_args
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        // Ordinary close releases native memory even when a receiver is still
        // retained. An entered apply owns its buffers until that effect settles.
        if !self.applying.load(Ordering::Acquire)
            && let Ok(mut session) = self.session.try_lock()
        {
            self.release(&mut session);
        }
        self.wake.notify_one();
    }
}

enum CallSession {
    Streamed(Arc<Shared>),
    Owned(Session),
}
/// The captured original session stays with the independent entered effect.
pub struct EditCall {
    session: CallSession,
    resolver: Arc<EditUrlResolver>,
    _cleanup: Option<Cleanup>,
    blackbox_serializer: Option<Arc<dyn crate::edit_post_apply::EditBlackboxSerializer>>,
    repair_dispatcher: Option<Arc<dyn crate::edit_post_apply::EditRepairCompletionDispatcher>>,
}
struct Cleanup {
    pool: Arc<SessionPool>,
    id: String,
    instance: Arc<Shared>,
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        self.instance.close();
        self.pool.remove_if_current(&self.id, &self.instance);
    }
}
impl SessionPool {
    fn remove_if_current(&self, id: &str, instance: &Arc<Shared>) {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(index) = entries
            .iter()
            .position(|(key, value)| key == id && Arc::ptr_eq(value, instance))
        {
            entries.remove(index);
        }
    }
    pub(super) fn open(
        self: &Arc<Self>,
        mut config: SessionConfig,
        store: ara_edit::EditStore,
        init: ToolArgStreamInit,
        resolver: Arc<EditUrlResolver>,
    ) -> Result<Box<dyn ToolArgStream>, ToolError> {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| ToolError("Edit preview requires the current async host".into()))?;
        config.raw_input = init.custom_wire_name.is_some();
        let instance = Arc::new(Shared {
            session: Mutex::new(Session::new(config, store)),
            resolver,
            queue: Mutex::new(Vec::new()),
            streamed_args: Mutex::new(None),
            closed: AtomicBool::new(false),
            applying: AtomicBool::new(false),
            wake: Notify::new(),
        });
        {
            let mut entries = self
                .entries
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(index) = entries.iter().position(|(id, _)| id == &init.tool_call_id) {
                let (_, previous) = entries.remove(index).expect("selected pool row");
                previous.close();
            }
            entries.push_back((init.tool_call_id.clone(), instance.clone()));
            while entries.len() > 32 {
                entries.pop_front().expect("oversized pool").1.close();
            }
        }
        runtime.spawn(preview_pump(instance.clone(), init.emit.clone()));
        Ok(Box::new(EditStream {
            pool: self.clone(),
            instance,
            id: init.tool_call_id,
        }))
    }
    pub(super) fn begin(self: &Arc<Self>, id: &str) -> Option<EditCall> {
        self.begin_inner(id, None)
    }
    pub(super) fn begin_matching(self: &Arc<Self>, id: &str, args_json: &str) -> Option<EditCall> {
        self.begin_inner(id, Some(args_json))
    }
    fn begin_inner(self: &Arc<Self>, id: &str, args_json: Option<&str>) -> Option<EditCall> {
        let instance = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .find(|(key, _)| key == id)
            .map(|(_, value)| value.clone())?;
        if let Some(args_json) = args_json {
            let finished = instance
                .streamed_args
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take();
            if finished.as_deref() != Some(args_json) {
                instance.close();
                self.remove_if_current(id, &instance);
                return None;
            }
        }
        Some(EditCall {
            session: CallSession::Streamed(instance.clone()),
            resolver: instance.resolver.clone(),
            _cleanup: Some(Cleanup {
                pool: self.clone(),
                id: id.into(),
                instance,
            }),
            blackbox_serializer: None,
            repair_dispatcher: None,
        })
    }
}
impl Drop for SessionPool {
    fn drop(&mut self) {
        for (_, instance) in self
            .entries
            .get_mut()
            .unwrap_or_else(|error| error.into_inner())
            .drain(..)
        {
            instance.close();
        }
    }
}
impl EditCall {
    pub(super) fn from_json(
        config: SessionConfig,
        store: ara_edit::EditStore,
        json: &str,
        resolver: Arc<EditUrlResolver>,
    ) -> Self {
        let mut session = Session::new(config, store);
        session.set_args_json(json);
        session.finish();
        Self {
            session: CallSession::Owned(session),
            resolver,
            _cleanup: None,
            blackbox_serializer: None,
            repair_dispatcher: None,
        }
    }
    pub fn set_blackbox_serializer(
        &mut self,
        serializer: Arc<dyn crate::edit_post_apply::EditBlackboxSerializer>,
    ) {
        self.blackbox_serializer = Some(serializer);
    }
    pub fn set_repair_dispatcher(
        &mut self,
        dispatcher: Arc<dyn crate::edit_post_apply::EditRepairCompletionDispatcher>,
    ) {
        self.repair_dispatcher = Some(dispatcher);
    }
    pub(super) fn repair_dispatcher(
        &self,
    ) -> Option<Arc<dyn crate::edit_post_apply::EditRepairCompletionDispatcher>> {
        self.repair_dispatcher.clone()
    }
    pub(super) fn blackbox_serializer(
        &self,
    ) -> Option<Arc<dyn crate::edit_post_apply::EditBlackboxSerializer>> {
        self.blackbox_serializer.clone()
    }
    /// Runs only inside the original independent blocking effect owner. Apply
    /// closes the preview pump, drains its original operations, then stages all.
    pub(super) fn apply(
        &mut self,
        request: ApplyRequest,
        writer: &dyn EditWriter,
        cancel: Option<CancellationToken>,
    ) -> EditResult<ara_edit::ApplyOutcome> {
        // Called only by EditTool's captured independent spawn_blocking owner.
        // Async Host resolution runs outside the std session mutex. The same
        // original Session is retained for every staging pass.
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| {
            ara_edit::EditError::apply("Edit URL resolution requires the current async Host")
        })?;
        if let CallSession::Streamed(shared) = &self.session {
            shared.applying.store(true, Ordering::Release);
            shared.close();
        }
        if let CallSession::Streamed(shared) = &self.session {
            let mut session = shared
                .session
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            shared.drain(&mut session);
        }
        let mut resolved = HashSet::new();
        let initial = self.with_session(|session| session.begin_apply_url_targets());
        self.provide_urls(&runtime, initial, &mut resolved, cancel.clone());
        loop {
            let outcome = self.with_session(|session| {
                futures::executor::block_on(session.apply(request.clone(), writer))
            });
            if matches!(outcome, Err(ara_edit::EditError::UnresolvedUrl(_))) {
                let misses = self.with_session(|session| session.take_unresolved());
                if self.provide_urls(&runtime, misses, &mut resolved, cancel.clone()) {
                    continue;
                }
            }
            if let CallSession::Streamed(shared) = &self.session {
                let mut session = shared
                    .session
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                shared.release(&mut session);
            }
            return outcome;
        }
    }
    fn with_session<R>(&mut self, f: impl FnOnce(&mut Session) -> R) -> R {
        match &mut self.session {
            CallSession::Owned(session) => f(session),
            CallSession::Streamed(shared) => f(&mut shared
                .session
                .lock()
                .unwrap_or_else(|error| error.into_inner())),
        }
    }
    fn provide_urls(
        &mut self,
        runtime: &tokio::runtime::Handle,
        urls: Vec<String>,
        resolved: &mut HashSet<String>,
        cancel: Option<CancellationToken>,
    ) -> bool {
        let mut provided = false;
        for url in urls {
            if !resolved.insert(url.clone()) {
                continue;
            }
            let answer = runtime.block_on(
                self.resolver
                    .resolve(&url, cancel.clone().unwrap_or_default()),
            );
            self.with_session(|session| session.provide(url, answer));
            provided = true;
        }
        provided
    }
}
struct EditStream {
    pool: Arc<SessionPool>,
    instance: Arc<Shared>,
    id: String,
}
impl ToolArgStream for EditStream {
    fn push(&mut self, delta: &str) -> Result<(), ToolError> {
        self.instance.enqueue(ArgOp::Push(delta.into()));
        Ok(())
    }
    fn end(&mut self, args: &JsonObject) -> Result<(), ToolError> {
        self.instance.enqueue(ArgOp::Finish);
        let encoded = serde_json::to_string(args)?;
        *self
            .instance
            .streamed_args
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(encoded);
        Ok(())
    }
    fn end_actual(&mut self, args: &ValueHandle, values: &SessionValues) -> Result<(), ToolError> {
        self.instance.enqueue(ArgOp::Finish);
        values.check_handle(args)?;
        // This is the original provider-final args object in its own realm.
        // End errors remain advisory at the existing argument-stream consumer.
        let encoded = values
            .heap()
            .stringify(args, StringifyMode::Plain, None, values.runtime())?
            .map(|text| text.to_utf8())
            .transpose()?;
        *self
            .instance
            .streamed_args
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = encoded;
        Ok(())
    }
    fn cancel(&mut self) -> Result<(), ToolError> {
        self.instance.close();
        self.pool.remove_if_current(&self.id, &self.instance);
        Ok(())
    }
}
async fn preview_pump(shared: Arc<Shared>, emit: ara_agent::ToolArgStreamEmit) {
    loop {
        shared.wake.notified().await;
        if shared.closed.load(Ordering::Acquire) {
            return;
        }
        let mut resolved = HashSet::new();
        let batch = loop {
            let work = shared.clone();
            let preview = tokio::task::spawn_blocking(move || {
                let mut session = work
                    .session
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if work.stop_previewing(&mut session) {
                    return None;
                }
                work.drain(&mut session);
                if !session.preview_pending() {
                    return None;
                }
                let batch = session.preview();
                if work.stop_previewing(&mut session) {
                    return None;
                }
                Some((batch, session.take_unresolved()))
            })
            .await;
            let (batch, mut misses) = match preview {
                Ok(Some(pass)) => pass,
                Ok(None) => break None,
                Err(_) => return,
            };
            misses.retain(|url| resolved.insert(url.clone()));
            if misses.is_empty() {
                break Some(batch);
            }
            let mut answers = Vec::new();
            for url in misses {
                if shared.closed.load(Ordering::Acquire) {
                    return;
                }
                let answer = shared
                    .resolver
                    .resolve(&url, CancellationToken::new())
                    .await;
                answers.push((url, answer));
            }
            let mut session = shared
                .session
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if shared.stop_previewing(&mut session) {
                return;
            }
            for (url, answer) in answers {
                session.provide(url, answer);
            }
        };
        let Some(batch) = batch else {
            continue;
        };
        if shared.closed.load(Ordering::Acquire) {
            return;
        }
        let final_pass = !batch.streaming;
        // Fixed native pump skips empty transient passes but always delivers
        // the final pass, including an empty final batch.
        if batch.files.is_empty() && !final_pass {
            continue;
        }
        // One callback is in flight; no locks are held during Host invocation.
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| emit(batch_json(batch))))
            .is_err()
        {
            #[cfg(debug_assertions)]
            eprintln!("ara: native Edit preview consumer failed");
            return;
        }
        if final_pass {
            return;
        }
    }
}
fn batch_json(batch: ara_edit::PreviewBatch) -> Value {
    let files: Vec<Value> = batch
        .files
        .into_iter()
        .map(|file| {
            let mut fields = serde_json::Map::new();
            fields.insert("path".into(), file.display.into());
            if let Some(diff) = file.diff {
                fields.insert("diff".into(), diff.into());
            }
            if let Some(error) = file.error {
                fields.insert("error".into(), error.into());
            }
            if let Some(line) = file.first_changed_line {
                fields.insert("firstChangedLine".into(), line.into());
            }
            if let Some(rename) = file.rename {
                fields.insert("rename".into(), rename.into());
            }
            if let Some(op) = file.op {
                fields.insert("op".into(), op.as_str().into());
            }
            Value::Object(fields)
        })
        .collect();
    json!({"generation":batch.generation,"streaming":batch.streaming,"files":files})
}
