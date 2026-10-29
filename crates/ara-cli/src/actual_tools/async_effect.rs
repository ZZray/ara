//! Native effect settlement is independent of the revocable graph Run.
//! Workers and joins own only native actions/payloads. Graph records below are
//! checked edges + sealed parent/scope metadata and are driven synchronously.

use super::*;
use crate::native_realm::{
    context::NativeScope,
    parent::{NativeParentId, NativeParentLease},
    promise::NativePromiseAdmission,
};
use ara_rpc::value_heap::DeferredValue;
use futures::{FutureExt, future::Shared};
use std::{
    collections::{HashSet, VecDeque},
    panic::AssertUnwindSafe,
};
use tokio::sync::Notify;
mod repair;
mod read_delimited;
#[cfg(test)]
mod repair_tests;

#[derive(Clone, Debug)]
pub enum NativeToolFault {
    Kernel(String),
    /// An authoritative native adapter observed that no effect was published.
    NotDispatched(String),
    OutcomeUnknown(String),
    /// Original Bash rejection text; native settlement is a separate receipt.
    BashCancelled(String),
    Cancelled,
    Panicked(String),
    JoinCancelled,
    JoinPanicked,
    Closed,
    SequenceExhausted,
}
impl NativeToolFault {
    fn tag(&self) -> &'static str {
        match self {
            Self::Kernel(_) => "kernel_failed",
            Self::NotDispatched(_) => "not_dispatched",
            Self::OutcomeUnknown(_) => "outcome_unknown",
            Self::BashCancelled(_) | Self::Cancelled => "cancelled",
            Self::Panicked(_) => "worker_panicked",
            Self::JoinCancelled => "join_cancelled",
            Self::JoinPanicked => "join_panicked",
            Self::Closed => "closed",
            Self::SequenceExhausted => "sequence_exhausted",
        }
    }
    pub(super) fn message(&self) -> &str {
        match self {
            Self::Kernel(text)
            | Self::NotDispatched(text)
            | Self::OutcomeUnknown(text)
            | Self::BashCancelled(text)
            | Self::Panicked(text) => text,
            Self::Cancelled => "Aborted: Cancelled",
            Self::JoinCancelled => "Native effect task was cancelled before a completion receipt",
            Self::JoinPanicked => "Native effect task panicked before a completion receipt",
            Self::Closed => "Native effect admission is closed",
            Self::SequenceExhausted => "Native effect sequence is exhausted",
        }
    }
}
pub type NativeEffectFuture = BoxFuture<'static, Result<ToolOutput, NativeToolFault>>;
type NativeJoin = Shared<BoxFuture<'static, Result<(), NativeToolFault>>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HostEffectId(u64);
#[derive(Clone, Debug)]
pub struct HostEffectReceipt {
    id: HostEffectId,
    entered: bool,
    cancelled: bool,
    failure: Option<NativeToolFault>,
    tool_is_error: bool,
    unknown: bool,
    authoritative_complete: bool,
    bash_process_settled: bool,
}
impl HostEffectReceipt {
    pub fn failed(&self) -> bool {
        self.failure.is_some() || self.tool_is_error
    }
    /// Exact native Bash receipt, or trusted proof that no effect launched.
    /// Generic certainty alone cannot authorize contained-process settlement.
    pub fn confirmed_bash_settlement(&self) -> bool {
        !self.uncertain()
            && (self.bash_process_settled || matches!(&self.failure, Some(NativeToolFault::NotDispatched(_))))
    }
    pub fn uncertain(&self) -> bool {
        self.entered
            && (self.unknown
                || (self.cancelled
                    && !self.authoritative_complete
                    && !self.bash_process_settled
                    && !matches!(&self.failure, Some(NativeToolFault::NotDispatched(_)))))
    }
    /// Native identity/status only: never output, args, fault messages or secrets.
    pub fn summary(&self) -> String {
        format!(
            "Host effect {}: {}{}",
            self.id.0,
            self.failure
                .as_ref()
                .map_or(if self.tool_is_error { "tool_is_error" } else { "completed" }, NativeToolFault::tag),
            if self.uncertain() { " (effects may be unknown)" } else { "" }
        )
    }
}
enum NativeNoticeKind {
    AstGrepLanguage {
        explicit:bool,
        reply:tokio::sync::oneshot::Sender<Result<Option<String>,NativeToolFault>>,
    },
    ReadDelimited {
        parts: Vec<String>,
        reply: tokio::sync::oneshot::Sender<Result<(), NativeToolFault>>,
    },
    ReadDelimitedClosed,
    ReadAttachmentLookup {
        uri: String,
        reply: tokio::sync::oneshot::Sender<Result<String, NativeToolFault>>,
    },
    // Settled repair notices keep their original sequence position.
    EditRepairClosed,
    EditRepairComplete {
        completion: Arc<dyn ara_tools::edit_post_apply::EditRepairCompletion>,
        prompt: String,
        cancel: CancellationToken,
        reply: tokio::sync::oneshot::Sender<Result<String, String>>,
    },
    WriteBatchRequest {
        reply: tokio::sync::oneshot::Sender<Result<Option<ara_tools::edit_host::EditLspBatch>, String>>,
    },
    Partial(ToolOutput),
    WriteProgress {
        output: ToolOutput,
        reply: tokio::sync::oneshot::Sender<bool>,
    },
    Complete(Result<ToolOutput, NativeToolFault>),
    EditBlackboxSerialize {
        record: ara_tools::edit_post_apply::EditBlackboxRecord,
        reply: tokio::sync::oneshot::Sender<Result<String, String>>,
    },
}
struct NativeNotice {
    id: HostEffectId,
    sequence: u64,
    kind: NativeNoticeKind,
}
enum ReadPublication {
    Output(ara_rpc::WireValue),
    TypeError(String),
}
struct EffectEntry {
    read_publication: Option<ReadPublication>,
    entered: bool,
    next_sequence: u64,
    cancel: Option<CancellationToken>,
    join: Option<NativeJoin>,
    receipt: Option<HostEffectReceipt>,
}
struct EffectState {
    sealed: bool,
    next_id: u64,
    entries: HashMap<HostEffectId, EffectEntry>,
    notices: VecDeque<NativeNotice>,
}
pub struct HostEffectSettlementOwner {
    state: Mutex<EffectState>,
    wake: Arc<Notify>,
}
impl HostEffectSettlementOwner {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(EffectState {
                sealed: false,
                next_id: 1,
                entries: HashMap::new(),
                notices: VecDeque::new(),
            }),
            wake: Arc::new(Notify::new()),
        }
    }
    pub fn wake(&self) -> Arc<Notify> {
        self.wake.clone()
    }
    pub fn seal(&self) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).sealed = true;
        self.wake.notify_waiters();
    }
    pub fn cancel(&self) {
        let tokens = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.entries.values_mut().filter_map(|entry| {
                // A revoked graph never leaves an exact output waiting for a
                // future Run or a worker's late completion.
                entry.read_publication = None;
                entry.cancel.clone()
            }).collect::<Vec<_>>()
        };
        for token in tokens {
            token.cancel();
        }
        self.wake.notify_waiters();
    }
    pub fn pending_count(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entries
            .values()
            .filter(|entry| entry.receipt.is_none())
            .count()
    }
    pub fn reserve(self: &Arc<Self>) -> Result<HostEffectReservation, NativeToolFault> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.sealed {
            return Err(NativeToolFault::Closed);
        }
        let id = HostEffectId(state.next_id);
        state.next_id = state.next_id.checked_add(1).ok_or(NativeToolFault::SequenceExhausted)?;
        state
            .entries
            .insert(id, EffectEntry { read_publication: None, entered: false, next_sequence: 0, cancel: None, join: None, receipt: None });
        Ok(HostEffectReservation { owner: self.clone(), id, attached: false })
    }
    fn publish_read(&self, id: HostEffectId, publication: ReadPublication) -> Result<(), NativeToolFault> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.sealed { return Err(NativeToolFault::Closed); }
        let entry = state.entries.get_mut(&id).ok_or(NativeToolFault::Closed)?;
        if !entry.entered || entry.receipt.is_some() || entry.cancel.as_ref().is_some_and(CancellationToken::is_cancelled) {
            return Err(NativeToolFault::Closed);
        }
        if entry.read_publication.is_some() { return Err(NativeToolFault::Kernel("READ exact completion was already published".into())); }
        entry.read_publication = Some(publication);
        Ok(())
    }
    fn take_read_publication(&self, id: HostEffectId) -> Option<ReadPublication> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).entries.get_mut(&id)?.read_publication.take()
    }
    fn partial(&self, id: HostEffectId, output: ToolOutput) -> Result<(), NativeToolFault> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let entry = state.entries.get_mut(&id).ok_or(NativeToolFault::Closed)?;
        if !entry.entered || entry.receipt.is_some() {
            return Err(NativeToolFault::Closed);
        }
        let sequence = entry.next_sequence;
        entry.next_sequence = sequence.checked_add(1).ok_or(NativeToolFault::SequenceExhausted)?;
        state.notices.push_back(NativeNotice { id, sequence, kind: NativeNoticeKind::Partial(output) });
        drop(state);
        self.wake.notify_waiters();
        Ok(())
    }
    fn write_progress(
        &self,
        id: HostEffectId,
        output: ToolOutput,
        reply: tokio::sync::oneshot::Sender<bool>,
    ) -> Result<CancellationToken, NativeToolFault> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let entry = state.entries.get_mut(&id).ok_or(NativeToolFault::Closed)?;
        if !entry.entered || entry.receipt.is_some() {
            return Err(NativeToolFault::Closed);
        }
        let cancel = entry.cancel.clone().ok_or(NativeToolFault::Closed)?;
        let sequence = entry.next_sequence;
        entry.next_sequence = sequence.checked_add(1).ok_or(NativeToolFault::SequenceExhausted)?;
        state.notices.push_back(NativeNotice { id, sequence, kind: NativeNoticeKind::WriteProgress { output, reply } });
        drop(state);
        self.wake.notify_waiters();
        Ok(cancel)
    }
    fn read_delimited(&self, id: HostEffectId, parts: Vec<String>, reply: tokio::sync::oneshot::Sender<Result<(), NativeToolFault>>) -> Result<(), NativeToolFault> {
        let mut state = self.state.lock().unwrap_or_else(|e|e.into_inner());
        if state.sealed { return Err(NativeToolFault::Closed); }
        let entry = state.entries.get_mut(&id).ok_or(NativeToolFault::Closed)?;
        if !entry.entered || entry.receipt.is_some() { return Err(NativeToolFault::Closed); }
        let sequence = entry.next_sequence;
        entry.next_sequence = sequence.checked_add(1).ok_or(NativeToolFault::SequenceExhausted)?;
        state.notices.push_back(NativeNotice { id, sequence, kind: NativeNoticeKind::ReadDelimited { parts, reply } });
        drop(state);self.wake.notify_waiters();Ok(())
    }
    fn read_attachment_lookup(&self, id: HostEffectId, uri: String, reply: tokio::sync::oneshot::Sender<Result<String, NativeToolFault>>) -> Result<CancellationToken, NativeToolFault> {
        let mut state = self.state.lock().unwrap_or_else(|e|e.into_inner());
        let entry = state.entries.get_mut(&id).ok_or(NativeToolFault::Closed)?;
        if !entry.entered || entry.receipt.is_some() { return Err(NativeToolFault::Closed); }
        let cancel = entry.cancel.clone().ok_or(NativeToolFault::Closed)?;
        let sequence = entry.next_sequence;
        entry.next_sequence = sequence.checked_add(1).ok_or(NativeToolFault::SequenceExhausted)?;
        state.notices.push_back(NativeNotice{id,sequence,kind:NativeNoticeKind::ReadAttachmentLookup{uri,reply}});
        drop(state);self.wake.notify_waiters();Ok(cancel)
    }
    fn ast_grep_language(&self,id:HostEffectId,explicit:bool,reply:tokio::sync::oneshot::Sender<Result<Option<String>,NativeToolFault>>)->Result<CancellationToken,NativeToolFault> {
        let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());
        let entry=state.entries.get_mut(&id).ok_or(NativeToolFault::Closed)?;
        if !entry.entered || entry.receipt.is_some() {return Err(NativeToolFault::Closed);}
        let cancel=entry.cancel.clone().ok_or(NativeToolFault::Closed)?;
        let sequence=entry.next_sequence;entry.next_sequence=sequence.checked_add(1).ok_or(NativeToolFault::SequenceExhausted)?;
        state.notices.push_back(NativeNotice {id,sequence,kind:NativeNoticeKind::AstGrepLanguage {explicit,reply}});
        drop(state);self.wake.notify_waiters();Ok(cancel)
    }
    fn write_batch_request(
        &self,
        id: HostEffectId,
        reply: tokio::sync::oneshot::Sender<Result<Option<ara_tools::edit_host::EditLspBatch>, String>>,
    ) -> Result<CancellationToken, NativeToolFault> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let entry = state.entries.get_mut(&id).ok_or(NativeToolFault::Closed)?;
        if !entry.entered || entry.receipt.is_some() {
            return Err(NativeToolFault::Closed);
        }
        let cancel = entry.cancel.clone().ok_or(NativeToolFault::Closed)?;
        let sequence = entry.next_sequence;
        entry.next_sequence = sequence.checked_add(1).ok_or(NativeToolFault::SequenceExhausted)?;
        state.notices.push_back(NativeNotice { id, sequence, kind: NativeNoticeKind::WriteBatchRequest { reply } });
        drop(state);
        self.wake.notify_waiters();
        Ok(cancel)
    }
    fn edit_blackbox_serialize(
        &self,
        id: HostEffectId,
        record: ara_tools::edit_post_apply::EditBlackboxRecord,
        reply: tokio::sync::oneshot::Sender<Result<String, String>>,
    ) -> Result<CancellationToken, NativeToolFault> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let entry = state.entries.get_mut(&id).ok_or(NativeToolFault::Closed)?;
        if !entry.entered || entry.receipt.is_some() {
            return Err(NativeToolFault::Closed);
        }
        let cancel = entry.cancel.clone().ok_or(NativeToolFault::Closed)?;
        let sequence = entry.next_sequence;
        entry.next_sequence = sequence.checked_add(1).ok_or(NativeToolFault::SequenceExhausted)?;
        state.notices.push_back(NativeNotice {
            id,
            sequence,
            kind: NativeNoticeKind::EditBlackboxSerialize { record, reply },
        });
        drop(state);
        self.wake.notify_waiters();
        Ok(cancel)
    }
    fn edit_repair_complete(
        &self, id: HostEffectId,
        completion: Arc<dyn ara_tools::edit_post_apply::EditRepairCompletion>,
        prompt: String, cancel: CancellationToken,
        reply: tokio::sync::oneshot::Sender<Result<String, String>>,
    ) -> Result<(), NativeToolFault> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.sealed { return Err(NativeToolFault::Closed); }
        let entry = state.entries.get_mut(&id).ok_or(NativeToolFault::Closed)?;
        if !entry.entered || entry.receipt.is_some() { return Err(NativeToolFault::Closed); }
        let sequence = entry.next_sequence;
        entry.next_sequence = sequence.checked_add(1).ok_or(NativeToolFault::SequenceExhausted)?;
        state.notices.push_back(NativeNotice { id, sequence, kind: NativeNoticeKind::EditRepairComplete { completion, prompt, cancel, reply } });
        drop(state);
        self.wake.notify_waiters();
        Ok(())
    }
    fn complete(&self, id: HostEffectId, output: Result<ToolOutput, NativeToolFault>, unknown: bool) {
        self.complete_with_receipt(id, output, unknown, false, false);
    }
    fn complete_with_receipt(
        &self,
        id: HostEffectId,
        output: Result<ToolOutput, NativeToolFault>,
        unknown: bool,
        authoritative_complete: bool,
        bash_process_settled: bool,
    ) {
        let authoritative_complete = authoritative_complete && output.is_ok();
        // Only enter_bash supplies this native fact. Tool text/details and
        // foreign errors cannot convert an uncertain effect into completion.
        let bash_process_settled =
            bash_process_settled && matches!(&output, Ok(_) | Err(NativeToolFault::BashCancelled(_)));
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(entry) = state.entries.get_mut(&id) else {
            return;
        };
        if entry.receipt.is_some() {
            return;
        }
        let sequence = entry.next_sequence;
        let cancelled = entry.cancel.as_ref().is_some_and(CancellationToken::is_cancelled);
        if cancelled || output.is_err() && matches!(entry.read_publication, Some(ReadPublication::Output(_))) {
            entry.read_publication = None;
        }
        entry.receipt = Some(HostEffectReceipt {
            id,
            entered: entry.entered,
            cancelled,
            failure: output.as_ref().err().cloned(),
            tool_is_error: output.as_ref().is_ok_and(|output| output.is_error),
            unknown,
            authoritative_complete,
            bash_process_settled,
        });
        state.notices.push_back(NativeNotice { id, sequence, kind: NativeNoticeKind::Complete(output) });
        drop(state);
        self.wake.notify_waiters();
    }
    fn release(&self, id: HostEffectId) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = state.entries.get_mut(&id) {
            if !entry.entered && entry.receipt.is_none() {
                entry.receipt = Some(HostEffectReceipt {
                    id,
                    entered: false,
                    cancelled: false,
                    failure: Some(NativeToolFault::Closed),
                    tool_is_error: false,
                    unknown: false,
                    authoritative_complete: false,
                    bash_process_settled: false,
                });
            }
        }
        drop(state);
        self.wake.notify_waiters();
    }
    /// Shared futures stay in this owner. Dropping one settle caller cannot
    /// drop the last JoinHandle or make an entered effect appear undone.
    pub async fn settle(&self) -> Vec<HostEffectReceipt> {
        loop {
            let wake = self.wake();
            let notified = wake.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let (joins, pending) = {
                let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                (
                    state.entries.values().filter_map(|entry| entry.join.clone()).collect::<Vec<_>>(),
                    state.entries.values().any(|entry| entry.receipt.is_none()),
                )
            };
            for join in joins {
                let _ = join.await;
            }
            if self.pending_count() == 0 {
                return self
                    .state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .entries
                    .values()
                    .filter_map(|entry| entry.receipt.clone())
                    .collect();
            }
            if pending {
                notified.await;
            }
        }
    }
    /// Await one already-admitted native effect without waiting on unrelated
    /// Session jobs. Its shared join remains retained when one waiter drops.
    pub(crate) async fn settle_one(&self, id: HostEffectId) -> Option<HostEffectReceipt> {
        let join =
            self.state.lock().unwrap_or_else(|e| e.into_inner()).entries.get(&id).and_then(|entry| entry.join.clone());
        if let Some(join) = join {
            let _ = join.await;
        }
        self.state.lock().unwrap_or_else(|e| e.into_inner()).entries.get(&id).and_then(|entry| entry.receipt.clone())
    }
    /// A native adapter claims only its own complete notice; it cannot steal
    /// another adapter's FIFO or manufacture a graph completion.
    pub(crate) fn take_completion_for(&self, id: HostEffectId) -> Option<Result<ToolOutput, NativeToolFault>> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let position = state
            .notices
            .iter()
            .position(|notice| { notice.id == id && matches!(notice.kind, NativeNoticeKind::Complete(_))
        })?;
        let notice = state.notices.remove(position)?;
        match notice.kind {
            NativeNoticeKind::Complete(result) => Some(result),
            _ => None,
        }
    }
    fn take_notice(&self) -> Option<NativeNotice> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).notices.pop_front()
    }
    fn has_ready(&self) -> bool {
        !self.state.lock().unwrap_or_else(|e| e.into_inner()).notices.is_empty()
    }
}
impl Default for HostEffectSettlementOwner {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
pub struct NativeEffectPublisher {
    owner: Arc<HostEffectSettlementOwner>,
    id: HostEffectId,
}
impl NativeEffectPublisher {
    pub(super) fn publish_read_output(&self, output: ara_rpc::WireValue) -> Result<(), NativeToolFault> {
        self.owner.publish_read(self.id, ReadPublication::Output(output))
    }
    pub(super) fn publish_read_type_error(&self, message: String) -> Result<(), NativeToolFault> {
        self.owner.publish_read(self.id, ReadPublication::TypeError(message))
    }
    pub(super) async fn read_delimited(&self, parts: Vec<String>) -> Result<(), NativeToolFault> {
        let (reply, response) = tokio::sync::oneshot::channel();
        self.owner.read_delimited(self.id, parts, reply)?;
        // A cancellation token alone cannot replace the original child throw.
        // Explicit graph settlement closes this reply before joining workers.
        response.await.unwrap_or(Err(NativeToolFault::Closed))
    }
    pub(super) async fn lookup_read_attachment(&self, uri: String) -> Result<String, NativeToolFault> {
        let (reply,response)=tokio::sync::oneshot::channel();
        let cancel=self.owner.read_attachment_lookup(self.id,uri,reply)?;
        tokio::select! {
            biased;
            _=cancel.cancelled()=>Err(NativeToolFault::Cancelled),
            result=response=>result.unwrap_or(Err(NativeToolFault::Closed)),
        }
    }
    pub async fn ast_grep_language(&self,explicit:bool)->Result<Option<String>,ToolError> {
        let (reply,response)=tokio::sync::oneshot::channel();
        let cancel=self.owner.ast_grep_language(self.id,explicit,reply).map_err(|e|ToolError(e.message().to_owned()))?;
        tokio::select! {biased;
            _=cancel.cancelled()=>Err(ToolError("Aborted: Cancelled".into())),
            result=response=>result.unwrap_or(Err(NativeToolFault::Closed)).map_err(|e|ToolError(e.message().to_owned())),
        }
    }
    /// Only native data crosses the worker boundary. The original callback
    /// and its exception stay on the admitted graph driver.
    pub async fn acknowledge_write_progress(&self, output: ToolOutput) -> Result<(), ToolError> {
        let (reply, response) = tokio::sync::oneshot::channel();
        let cancel =
            self.owner.write_progress(self.id, output, reply).map_err(|error| ToolError(error.message().to_owned()))?;
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(ToolError("Aborted: Cancelled".into())),
            result = response => match result {
                Ok(true) => Ok(()),
                Ok(false) => Err(ToolError("Original Writing update rejected before write".into())),
                Err(_) => Err(ToolError("Original Writing acknowledgement closed before write".into())),
            },
        }
    }
    /// Native scalars only; the original context/Get/throw stay on the serial
    /// admitted Graph scope, at the worker's fixed pre-write position.
    pub async fn prepare_write_batch(&self) -> Result<Option<ara_tools::edit_host::EditLspBatch>, ToolError> {
        let (reply, response) = tokio::sync::oneshot::channel();
        let cancel =
            self.owner.write_batch_request(self.id, reply).map_err(|error| ToolError(error.message().to_owned()))?;
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(ToolError("Aborted: Cancelled".into())),
            result = response => match result {
                Ok(Ok(batch)) => Ok(batch),
                Ok(Err(message)) => Err(ToolError(message)),
                Err(_) => Err(ToolError("Original WRITE batch acknowledgement closed before write".into())),
            },
        }
    }
    pub async fn stringify_edit_blackbox(
        &self,
        record: ara_tools::edit_post_apply::EditBlackboxRecord,
    ) -> Result<String, String> {
        let (reply, response) = tokio::sync::oneshot::channel();
        let cancel = self.owner.edit_blackbox_serialize(self.id, record, reply).map_err(|e| e.message().to_owned())?;
        tokio::select! {
            biased;
            _=cancel.cancelled()=>Err("Edit blackbox original realm cancelled".into()),
            result=response=>result.map_err(|_|"Edit blackbox original realm closed".to_owned())?,
        }
    }
    pub async fn complete_edit_repair(&self, completion: Arc<dyn ara_tools::edit_post_apply::EditRepairCompletion>, prompt: String, cancel: CancellationToken) -> Result<String, String> {
        let (reply, response) = tokio::sync::oneshot::channel();
        self.owner.edit_repair_complete(self.id, completion, prompt, cancel.clone(), reply).map_err(|error| error.message().to_owned())?;
        tokio::select! {
            biased;
            _=cancel.cancelled()=>Err("Edit repair original invocation cancelled".into()),
            result=response=>result.map_err(|_| "Edit repair original invocation closed".to_owned())?,
        }
    }
    pub fn partial(&self, output: ToolOutput) -> Result<(), NativeToolFault> {
        let result = self.owner.partial(self.id, output);
        if let Err(NativeToolFault::SequenceExhausted) = &result {
            self.owner.complete(self.id, Err(NativeToolFault::SequenceExhausted), true);
        }
        result
    }
}
pub struct HostEffectReservation {
    owner: Arc<HostEffectSettlementOwner>,
    id: HostEffectId,
    attached: bool,
}
impl HostEffectReservation {
    pub(crate) fn id(&self) -> HostEffectId {
        self.id
    }
    pub fn publisher(&self) -> NativeEffectPublisher {
        NativeEffectPublisher { owner: self.owner.clone(), id: self.id }
    }
    pub fn enter(self, cancel: CancellationToken, future: NativeEffectFuture) -> Result<(), NativeToolFault> {
        self.enter_with_receipt(cancel, future, None, None, false)
    }
    /// Only the trusted Bash runner supplies native deadline/contained-process
    /// facts. Original/foreign ToolOutput details are never settlement evidence.
    pub(crate) fn enter_bash(
        self,
        cancel: CancellationToken,
        future: NativeEffectFuture,
        timeout_unknown: Arc<std::sync::atomic::AtomicBool>,
        process_settled: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<(), NativeToolFault> {
        self.enter_with_receipt(cancel, future, Some(timeout_unknown), Some(process_settled), false)
    }
    /// Only RPC authoritative wire Complete may suppress cancellation-derived Unknown.
    /// A tool error remains failed; worker faults retain conservative knowledge.
    pub(crate) fn enter_authoritative(
        self,
        cancel: CancellationToken,
        future: NativeEffectFuture,
    ) -> Result<(), NativeToolFault> {
        self.enter_with_receipt(cancel, future, None, None, true)
    }
    fn enter_with_receipt(
        mut self,
        cancel: CancellationToken,
        future: NativeEffectFuture,
        timeout_unknown: Option<Arc<std::sync::atomic::AtomicBool>>,
        bash_process_settled: Option<Arc<std::sync::atomic::AtomicBool>>,
        authoritative_success: bool,
    ) -> Result<(), NativeToolFault> {
        let owner = self.owner.clone();
        let id = self.id;
        let mut state = owner.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.sealed {
            return Err(NativeToolFault::Closed);
        }
        let entry = state.entries.get_mut(&id).ok_or(NativeToolFault::Closed)?;
        if entry.entered || entry.receipt.is_some() {
            return Err(NativeToolFault::Closed);
        }
        entry.entered = true;
        entry.cancel = Some(cancel);
        let worker_owner = owner.clone();
        let join = tokio::spawn(async move {
            let outcome = AssertUnwindSafe(future).catch_unwind().await.unwrap_or_else(|panic| {
                let message = panic
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                    .unwrap_or("native worker panic payload");
                Err(NativeToolFault::Panicked(message.to_owned()))
            });
            let unknown = matches!(outcome, Err(NativeToolFault::Panicked(_) | NativeToolFault::OutcomeUnknown(_)))
                || timeout_unknown.as_ref().is_some_and(|flag| flag.load(Ordering::Acquire));
            let bash_process_settled = bash_process_settled.as_ref().is_some_and(|flag| flag.load(Ordering::Acquire));
            worker_owner.complete_with_receipt(id, outcome, unknown, authoritative_success, bash_process_settled);
        });
        // This native monitor consumes JoinError even while the Agent waits
        // on an authoritative graph ticket. It cannot depend on settle being
        // first called after that Agent finishes, or a lost worker would hang.
        let monitor_owner = owner.clone();
        let monitor = tokio::spawn(async move {
            match join.await {
                Ok(()) => Ok(()),
                Err(error) => {
                    let fault = if error.is_cancelled() {
                        NativeToolFault::JoinCancelled
                    } else {
                        NativeToolFault::JoinPanicked
                    };
                    monitor_owner.complete(id, Err(fault.clone()), true);
                    Err(fault)
                }
            }
        });
        let weak_owner = Arc::downgrade(&owner);
        entry.join = Some(
            async move {
                match monitor.await {
                    Ok(result) => result,
                    Err(error) => {
                        let fault = if error.is_cancelled() {
                            NativeToolFault::JoinCancelled
                        } else {
                            NativeToolFault::JoinPanicked
                        };
                        if let Some(owner) = weak_owner.upgrade() {
                            owner.complete(id, Err(fault.clone()), true);
                        }
                        Err(fault)
                    }
                }
            }
            .boxed()
            .shared(),
        );
        self.attached = true;
        drop(state);
        owner.wake.notify_waiters();
        Ok(())
    }
}
impl Drop for HostEffectReservation {
    fn drop(&mut self) {
        if !self.attached {
            self.owner.release(self.id);
        }
    }
}

struct ContextRecord {
    scope: NativeScope,
    parent: NativeParentId,
}
struct GraphJob {
    read_parent: Option<Weak<InvocationRuntime>>,
    scope: NativeScope,
    parent: NativeParentId,
    port: DeferredValue,
    next_sequence: u64,
    _lease: NativeParentLease,
}
#[derive(Clone)]
enum StoredValueFailure {
    Thrown(DeferredValue),
    Close(Box<StoredValueFailure>, Box<StoredValueFailure>),
    Other(ValueError),
}
impl StoredValueFailure {
    fn capture(values: &SessionValues, error: ValueError) -> Result<Self, ValueError> {
        Ok(match error {
            ValueError::Thrown(value) => Self::Thrown(values.heap().capture_deferred(&value)?),
            ValueError::IteratorClose { original, close } =>
                Self::Close(Box::new(Self::capture(values, *original)?), Box::new(Self::capture(values, *close)?),
            ),
            other => Self::Other(other),
        })
    }
    fn restore(&self, values: &SessionValues) -> Result<ValueError, ValueError> {
        Ok(match self {
            Self::Thrown(edge) => ValueError::Thrown(values.heap().restore_deferred(edge)?),
            Self::Close(original, close) => ValueError::IteratorClose {
                original: Box::new(original.restore(values)?),
                close: Box::new(close.restore(values)?),
            },
            Self::Other(error) => error.clone(),
        })
    }
}
#[derive(Clone)]
enum StoredAdmissionFailure {
    Value(StoredValueFailure),
    Other(ara_agent::GraphAdmissionError),
}
impl StoredAdmissionFailure {
    fn capture(values: &SessionValues, error: ara_agent::GraphAdmissionError) -> Result<Self, ValueError> {
        Ok(match error {
            ara_agent::GraphAdmissionError::Value(error) => { Self::Value(StoredValueFailure::capture(values, error)?)
            }
            other => Self::Other(other),
        })
    }
    fn restore(&self, values: &SessionValues) -> Result<ara_agent::GraphAdmissionError, ValueError> {
        Ok(match self {
            Self::Value(error) => ara_agent::GraphAdmissionError::Value(error.restore(values)?),
            Self::Other(error) => error.clone(),
        })
    }
}
#[derive(Clone)]
enum StoredOwnerFailure {
    Native(StoredValueFailure),
    Admission { reason: StoredAdmissionFailure, callback: Option<StoredValueFailure> },
}
#[derive(Clone)]
enum StoredActualFailure {
    Value(StoredValueFailure),
    Owner(StoredOwnerFailure),
    Unsupported(&'static str),
    PreparationUnavailable,
    Protocol(&'static str),
    Callback(String),
    Compound(Box<StoredActualFailure>, Box<StoredActualFailure>),
}
impl StoredActualFailure {
    fn capture(values: &SessionValues, error: ActualProviderError) -> Result<Self, ValueError> {
        Ok(match error {
            ActualProviderError::Value(error) => { Self::Value(StoredValueFailure::capture(values, error)?)
            }
            ActualProviderError::Owner(ara_agent::GraphOwnerError::Native(error)) =>
                Self::Owner(StoredOwnerFailure::Native(StoredValueFailure::capture(values, error)?),
            ),
            ActualProviderError::Owner(ara_agent::GraphOwnerError::Admission { reason, callback_failure,
            }) =>
                Self::Owner(StoredOwnerFailure::Admission {
                    reason: StoredAdmissionFailure::capture(values, reason)?,
                    callback: callback_failure.map(|error| StoredValueFailure::capture(values, error)).transpose()?,
                }),
            ActualProviderError::Unsupported(reason) => Self::Unsupported(reason),
            ActualProviderError::PreparationUnavailable => Self::PreparationUnavailable,
            ActualProviderError::Protocol(reason) => Self::Protocol(reason),
            ActualProviderError::Callback(reason) => Self::Callback(reason),
            ActualProviderError::Compound { primary, secondary } => Self::Compound(
                Box::new(Self::capture(values, primary.as_ref().clone())?),
                Box::new(Self::capture(values, secondary.as_ref().clone())?),
            ),
        })
    }
    fn restore(&self, values: &SessionValues) -> Result<ActualProviderError, ValueError> {
        Ok(match self {
            Self::Value(error) => ActualProviderError::Value(error.restore(values)?),
            Self::Owner(StoredOwnerFailure::Native(error)) =>
                ActualProviderError::Owner(ara_agent::GraphOwnerError::Native(error.restore(values)?),
            ),
            Self::Owner(StoredOwnerFailure::Admission { reason, callback }) => {
                ActualProviderError::Owner(ara_agent::GraphOwnerError::Admission {
                    reason: reason.restore(values)?,
                    callback_failure: callback.as_ref().map(|error| error.restore(values)).transpose()?,
                })
            }
            Self::Unsupported(reason) => ActualProviderError::Unsupported(reason),
            Self::PreparationUnavailable => ActualProviderError::PreparationUnavailable,
            Self::Protocol(reason) => ActualProviderError::Protocol(reason),
            Self::Callback(reason) => ActualProviderError::Callback(reason.clone()),
            Self::Compound(primary, secondary) => { compound(primary.restore(values)?, secondary.restore(values)?)
            }
        })
    }
}
struct RunState {
    sealed: bool,
    revoked: bool,
    contexts: HashMap<DeferredValue, ContextRecord>,
    seen: HashSet<DeferredValue>,
    jobs: HashMap<HostEffectId, GraphJob>,
    fatal: Option<StoredActualFailure>,
}
pub enum NativeHostDrain {
    Idle,
    RanJob,
}
pub struct NativeBuiltinRun {
    values: Weak<SessionValues>,
    pub(super) admission: NativePromiseAdmission,
    effects: Arc<HostEffectSettlementOwner>,
    read: read_continuation::NativeReadContinuationEdges,
    pub(super) error_constructor: DeferredValue,
    read_history: Mutex<Option<super::read_attachments::CapturedHistory>>,
    state: Mutex<RunState>,
    retry_realm: OnceLock<Weak<crate::native_realm::NativeRealm>>,
    pending_repair: Mutex<repair::RepairQueue>,
    repair_wake: Arc<repair::RepairWake>,
    pending_read: Mutex<read_delimited::ReadQueue>,
    read_wake: Arc<read_delimited::ReadWake>,
    repair_resources: Arc<crate::retry_host::repair::RepairResourcesOwner>,
}
pub(super) struct ClaimedContext {
    pub scope: NativeScope,
    pub parent: NativeParentId,
}
impl NativeBuiltinRun {
    pub fn new(
        values: &Arc<SessionValues>,
        admission: NativePromiseAdmission,
        effects: Arc<HostEffectSettlementOwner>,
        read: &read_continuation::NativeReadContinuation,
        error_constructor: &ValueHandle,
    ) -> Result<Arc<Self>, ActualProviderError> {
        admission.check_values(values)?;
        admission.check_live()?;
        values.check_handle(error_constructor)?;
        let repair_wake = repair::RepairWake::new(effects.wake());
        let read_wake = read_delimited::ReadWake::new(effects.wake());
        Ok(Arc::new(Self {
            values: Arc::downgrade(values),
            admission,
            effects,
            read: read.capture()?,
            retry_realm: OnceLock::new(),
            pending_repair: Mutex::new(repair::RepairQueue::default()),
            repair_wake,
            pending_read: Mutex::new(read_delimited::ReadQueue::default()),
            read_wake,
            repair_resources: crate::retry_host::repair::RepairResourcesOwner::new(),
            error_constructor: values.heap().capture_deferred(error_constructor)?,
            read_history: Mutex::new(None),
            state: Mutex::new(RunState {
                sealed: false,
                revoked: false,
                contexts: HashMap::new(),
                seen: HashSet::new(),
                jobs: HashMap::new(),
                fatal: None,
            }),
        }))
    }
    pub(super) fn capture_read_history(&self,messages:&[ara_agent::AgentMessage],blobs:std::path::PathBuf,values:&SessionValues)->Result<(),ActualProviderError> {
        self.check_values(values)?;self.check_live()?;
        let captured=super::read_attachments::CapturedHistory::capture(messages,blobs,values)?;
        *self.read_history.lock().unwrap_or_else(|e|e.into_inner())=Some(captured);
        Ok(())
    }
    pub(super) fn read_history(&self)->Option<super::read_attachments::CapturedHistory> {
        self.read_history.lock().unwrap_or_else(|e|e.into_inner()).clone()
    }
    pub fn check_values(&self, values: &SessionValues) -> Result<(), ActualProviderError> {
        let original = self.values.upgrade().ok_or(ActualProviderError::Protocol("Native tool Run values expired"))?;
        if !std::ptr::eq(original.as_ref(), values) {
            return Err(ValueError::ForeignHeap.into());
        }
        self.admission.check_values(values)?;
        Ok(())
    }
    pub fn check_live(&self) -> Result<(), ActualProviderError> {
        self.admission.check_live()?;
        let (revoked, fatal) = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            (state.revoked, state.fatal.clone())
        };
        if revoked {
            return Err(ActualProviderError::Protocol("Native tool Run is revoked"));
        }
        if let Some(error) = fatal {
            let values =
                self.values.upgrade().ok_or(ActualProviderError::Protocol("Native fatal carrier realm expired"))?;
            return Err(error.restore(&values)?);
        }
        Ok(())
    }
    /// Read the saved failure after the Promise driver has revoked admission.
    /// Restoring the same Session carrier performs no Graph Get/Call and never
    /// reopens effect admission or consumes the original failure.
    pub(crate) fn retained_failure(&self) -> Result<Option<ActualProviderError>, ActualProviderError> {
        let fatal = self.state.lock().unwrap_or_else(|e| e.into_inner()).fatal.clone();
        let Some(error) = fatal else { return Ok(None) };
        let values =
            self.values.upgrade().ok_or(ActualProviderError::Protocol("Native fatal carrier realm expired"))?;
        Ok(Some(error.restore(&values)?))
    }
    pub fn wake(&self) -> Arc<Notify> {
        self.effects.wake()
    }
    pub fn has_ready(&self) -> bool {
        self.effects.has_ready() || self.repair_wake.is_ready() || self.read_wake.is_ready()
    }
    pub fn outstanding(&self) -> usize {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).jobs.len()
    }
    pub fn seal(&self) -> Result<(), ActualProviderError> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).sealed = true;
        self.effects.seal();
        self.wake().notify_waiters();
        Ok(())
    }
    pub fn revoke(&self) {
        let _ = self.prepare_repair_settlement();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.revoked = true;
        state.sealed = true;
        state.contexts.clear();
        state.jobs.clear();
        drop(state);
        self.effects.seal();
        self.effects.cancel();
        self.wake().notify_waiters();
        self.admission.wake().notify_waiters();
    }
    pub(super) fn fail(&self, error: ActualProviderError) {
        let stored = match self.values.upgrade() {
            Some(values) => StoredActualFailure::capture(&values, error)
                .unwrap_or_else(|error| { StoredActualFailure::Value(StoredValueFailure::Other(error))
            }),
            None => StoredActualFailure::Protocol("Native failure carrier realm expired"),
        };
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.fatal = Some(match state.fatal.take() {
            Some(primary) => StoredActualFailure::Compound(Box::new(primary), Box::new(stored)),
            None => stored,
        });
        drop(state);
        self.wake().notify_waiters();
        self.admission.wake().notify_waiters();
    }
    pub(super) fn bind_context(
        &self,
        values: &SessionValues,
        context: &ValueHandle,
    ) -> Result<(), ActualProviderError> {
        self.check_values(values)?;
        self.check_live()?;
        let edge = values.heap().capture_deferred(context)?;
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.sealed || state.revoked || !state.seen.insert(edge) {
            return Err(ActualProviderError::Protocol("Native fresh context was already bound or admission closed"));
        }
        state
            .contexts
            .insert(edge, ContextRecord { scope: self.admission.root_scope(), parent: self.admission.root_parent(),
            },
        );
        Ok(())
    }
    pub(super) fn bind_derived_context(
        &self,
        values: &SessionValues,
        context: &ValueHandle,
        effective: &dyn ValueRuntime,
    ) -> Result<(), ActualProviderError> {
        self.check_values(values)?;
        self.check_live()?;
        let scope = self.admission.capture_scope(effective)?;
        let parent = self.admission.capture_parent(effective)?;
        let edge = values.heap().capture_deferred(context)?;
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.sealed || state.revoked || !state.seen.insert(edge) {
            return Err(ActualProviderError::Protocol(
                "Native derived context was already bound or admission closed",
            ));
        }
        state.contexts.insert(edge, ContextRecord { scope, parent });
        Ok(())
    }
    pub(super) fn claim(
        &self,
        values: &SessionValues,
        context: &ValueHandle,
    ) -> Result<ClaimedContext, ActualProviderError> {
        self.check_values(values)?;
        self.check_live()?;
        let edge = values.heap().capture_deferred(context)?;
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.sealed || state.revoked {
            return Err(ActualProviderError::Protocol("Native Run context admission closed"));
        }
        let record = state
            .contexts
            .remove(&edge)
            .ok_or(ActualProviderError::Protocol("Native tool context is unbound or already claimed"))?;
        Ok(ClaimedContext { scope: record.scope, parent: record.parent })
    }
    pub(super) fn reserve(
        &self,
        values: &SessionValues,
        scope: NativeScope,
        parent: NativeParentId,
        port: &ValueHandle,
    ) -> Result<HostEffectReservation, ActualProviderError> {
        self.check_values(values)?;
        self.check_live()?;
        let port = values.heap().capture_deferred(port)?;
        let lease = self.admission.retain_parent(parent)?;
        let reservation = self
            .effects
            .reserve()
            .map_err(|_| ActualProviderError::Protocol("Native effect reservation unavailable"))?;
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.sealed || state.revoked {
            return Err(ActualProviderError::Protocol("Native effect reservation after close"));
        }
        state.jobs.insert(reservation.id, GraphJob { read_parent: None, scope, parent, port, next_sequence: 0, _lease: lease });
        Ok(reservation)
    }
    pub(super) fn release(&self, reservation: &HostEffectReservation) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).jobs.remove(&reservation.id);
    }
    pub(super) fn final_enter(
        &self,
        reservation: &HostEffectReservation,
        owner: &Arc<BuiltinToolSource>,
        index: usize,
        id: CapabilityId,
        request: &super::xdev_native::NativeMethodRequest,
        cancel: CancellationToken,
    ) -> Result<EffectPermit, ActualProviderError> {
        // Nested request admission checks this same Run; check before its lock.
        if request.is_read_part() { owner.check_read_part_source(index, request)?; } else { owner.check_exact(index, request)?; }
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.sealed || state.revoked || !state.jobs.contains_key(&reservation.id) {
            return Err(ActualProviderError::Protocol("Native effect epoch closed before final grant"));
        }
        self.admission.check_live()?;
        // Keep final Run state and current source grant jointly protected.
        owner.enter_granted(index, id, cancel)
    }
    pub(super) fn native_type_error(
        &self,
        values: &SessionValues,
        message: &str,
        runtime: &dyn ValueRuntime,
    ) -> Result<ValueHandle, ValueError> {
        self.read.native_type_error(values, message, runtime)
    }
    pub(super) fn after_inner(
        &self,
        values: &SessionValues,
        receiver: &ValueHandle,
        params: &ValueHandle,
    ) -> Result<ValueHandle, ValueError> {
        self.read.after_inner(values, receiver, params)
    }
    pub fn drain_one(&self, values: &SessionValues) -> Result<NativeHostDrain, ActualProviderError> {
        self.check_values(values)?;
        if self.read_wake.is_ready() {
            self.poll_delimited_reads()?;
            return Ok(NativeHostDrain::RanJob);
        }
        if self.repair_wake.is_ready() {
            self.poll_repairs()?;
            return Ok(NativeHostDrain::RanJob);
        }
        let Some(notice) = self.effects.take_notice() else {
            return Ok(NativeHostDrain::Idle);
        };
        let metadata = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.revoked {
                return Ok(NativeHostDrain::RanJob);
            }
            let job = state
                .jobs
                .get_mut(&notice.id)
                .ok_or(ActualProviderError::Protocol("Native effect notice has no admitted graph job"))?;
            if job.next_sequence != notice.sequence {
                return Err(ActualProviderError::Protocol("Native effect duplicate or out-of-order notice"));
            }
            job.next_sequence = job.next_sequence.checked_add(1).ok_or(ValueError::NodeIdsExhausted)?;
            (job.scope, job.parent, job.port)
        };
        // Diagnostic consumer, not a user partial or terminal result. Drive it
        // on this same admitted scope/parent and reply only native JSON bytes.
        let kind = match notice.kind {
            NativeNoticeKind::ReadDelimitedClosed => return Ok(NativeHostDrain::RanJob),
            NativeNoticeKind::ReadDelimited { parts, reply } => {
                self.queue_delimited_read(values, notice.id, notice.sequence, metadata.0, metadata.1, parts, reply)?;
                return Ok(NativeHostDrain::RanJob);
            }
            NativeNoticeKind::EditRepairClosed => return Ok(NativeHostDrain::RanJob),
            NativeNoticeKind::EditRepairComplete { completion, prompt, cancel, reply } => {
                self.queue_repair(values, notice.id, notice.sequence, metadata.0, metadata.1, completion, prompt, cancel, reply)?;
                return Ok(NativeHostDrain::RanJob);
            }
            NativeNoticeKind::AstGrepLanguage {explicit,reply}=>{
                if reply.is_closed() {return Ok(NativeHostDrain::RanJob);}
                let outcome=(|| {
                    self.check_live()?;
                    let effective=self.admission.scoped(metadata.0,metadata.1)?;
                    let heap=values.heap();let port=heap.restore_deferred(&metadata.2)?;
                    let returned=heap.call(&port,&port,&[heap.number(9.0)?,heap.boolean(explicit)?],&effective)?;
                    match heap.snapshot(&returned)? {
                        ValueSnapshot::Undefined|ValueSnapshot::Null=>Ok(None),
                        ValueSnapshot::String(text)=>Ok(Some(text.to_utf8().map_err(|_|ActualProviderError::Unsupported("AST language UTF16 boundary"))?)),
                        _=>Err(ActualProviderError::Protocol("AST language reply is not optional string")),
                    }
                })();
                match outcome {
                    Ok(language)=>{let _=reply.send(Ok(language));},
                    Err(error)=>{
                        let _=reply.send(Err(NativeToolFault::Kernel("Original AST language Get failed; original Graph carrier retained".into())));
                        if let Err(retained)=self.check_live() {return Err(retained);}
                        self.fail(error.clone());return Err(self.check_live().err().unwrap_or(error));
                    }
                }
                return Ok(NativeHostDrain::RanJob);
            }
            NativeNoticeKind::ReadAttachmentLookup { uri, reply } => {
                if reply.is_closed() { return Ok(NativeHostDrain::RanJob); }
                let outcome = (|| {
                    self.check_live()?;
                    let effective=self.admission.scoped(metadata.0,metadata.1)?;
                    let heap=values.heap();
                    let port=heap.restore_deferred(&metadata.2)?;
                    let result=heap.call(&port,&port,&[heap.number(8.0)?,heap.string(uri.into())?],&effective)?;
                    let path=heap.get(&result,&"path".into(),&effective)?;
                    if let ValueSnapshot::String(path)=heap.snapshot(&path)? {
                        return Ok(Ok(path.to_utf8().map_err(|_|ActualProviderError::Unsupported("Attachment OS path UTF16 boundary"))?));
                    }
                    let error=heap.get(&result,&"error".into(),&effective)?;
                    let ValueSnapshot::String(error)=heap.snapshot(&error)? else {return Err(ActualProviderError::Protocol("Attachment lookup native completion has no path/error"))};
                    Ok(Err(NativeToolFault::Kernel(error.to_utf8().map_err(|_|ActualProviderError::Unsupported("Attachment error UTF16 boundary"))?)))
                })();
                match outcome {
                    Ok(result)=>{let _=reply.send(result);}
                    Err(error)=>{
                        let _=reply.send(Err(NativeToolFault::Kernel("Original attachment lookup failed; original Graph carrier retained".into())));
                        // Kind8 saves callback failure. Pre-port errors are saved here.
                        if let Err(retained)=self.check_live(){return Err(retained)}
                        self.fail(error.clone());return Err(self.check_live().err().unwrap_or(error));
                    }
                }
                return Ok(NativeHostDrain::RanJob);
            }
            NativeNoticeKind::WriteBatchRequest { reply } => {
                if reply.is_closed() {
                    return Ok(NativeHostDrain::RanJob);
                }
                let outcome = (|| {
                    self.check_live()?;
                    let effective = self.admission.scoped(metadata.0, metadata.1)?;
                    let heap = values.heap();
                    let port = heap.restore_deferred(&metadata.2)?;
                    let returned = heap.call(&port, &port, &[heap.number(7.0)?, heap.undefined()?], &effective)?;
                    if matches!(heap.snapshot(&returned)?, ValueSnapshot::Undefined) {
                        return Ok(None);
                    }
                    let id = heap.get(&returned, &"id".into(), &effective)?;
                    let ValueSnapshot::String(id) = heap.snapshot(&id)? else {
                        return Err(ActualProviderError::Protocol("WRITE native batch ID reply is not a string"));
                    };
                    let id =
                        id.to_utf8().map_err(|_| { ActualProviderError::Unsupported("WRITE batch ID UTF16 boundary")
                    })?;
                    let flush = heap.get(&returned, &"flush".into(), &effective)?;
                    let ValueSnapshot::Boolean(flush) = heap.snapshot(&flush)? else {
                        return Err(ActualProviderError::Protocol("WRITE native batch flush reply is not boolean"));
                    };
                    Ok(Some(ara_tools::edit_host::EditLspBatch { id, flush }))
                })();
                let _ =
                    reply.send(outcome.clone().map_err(|_: ActualProviderError| {
                        "Original WRITE batch request rejected before write".to_owned()
                    }));
                if let Err(error) = outcome {
                    if let Err(retained) = self.check_live() {
                        return Err(retained);
                    }
                    self.fail(error.clone());
                    return Err(self.check_live().err().unwrap_or(error));
                }
                return Ok(NativeHostDrain::RanJob);
            }
            NativeNoticeKind::WriteProgress { output, reply } => {
                if reply.is_closed() {
                    return Ok(NativeHostDrain::RanJob);
                }
                let outcome = (|| {
                    self.check_live()?;
                    let effective = self.admission.scoped(metadata.0, metadata.1)?;
                    let heap = values.heap();
                    let port = heap.restore_deferred(&metadata.2)?;
                    let payload = output_graph(values, output)?;
                    heap.call(&port, &port, &[heap.number(6.0)?, payload], &effective)?;
                    Ok::<_, ActualProviderError>(())
                })();
                let _ = reply.send(outcome.is_ok());
                if let Err(error) = outcome {
                    // kind6 already retained full callback failures. Failures
                    // before that Call are retained here once, without replacing
                    // an original throw or compounding it with itself.
                    if let Err(retained) = self.check_live() {
                        return Err(retained);
                    }
                    self.fail(error.clone());
                    return Err(self.check_live().err().unwrap_or(error));
                }
                return Ok(NativeHostDrain::RanJob);
            }
            NativeNoticeKind::EditBlackboxSerialize { record, reply } => {
                if !reply.is_closed() {
                    let result = (|| {
                        self.check_live()?;
                        let effective = self.admission.scoped(metadata.0, metadata.1)?;
                        let heap = values.heap();
                        let port = heap.restore_deferred(&metadata.2)?;
                        let payload = heap.object()?;
                        for (key, value) in [
                            ("prev", record.prev),
                            ("new", record.next),
                            ("model", record.model),
                            ("variant", record.variant),
                        ] {
                            heap.define_own_data(
                                &payload,
                                PropertyKey::String(key.into()),
                                heap.string(value.into())?,
                                &effective,
                            )?;
                        }
                        let returned = heap.call(&port, &port, &[heap.number(5.0)?, payload], &effective)?;
                        let ValueSnapshot::String(text) = heap.snapshot(&returned)? else {
                            return Err(ActualProviderError::Protocol("Edit blackbox stringify returned non-string"));
                        };
                        text.to_utf8()
                            .map_err(|_| { ActualProviderError::Unsupported("Edit blackbox JSON UTF16 boundary")
                        })
                    })();
                    let _ = reply.send(result.map_err(|e: ActualProviderError| e.to_string()));
                }
                return Ok(NativeHostDrain::RanJob);
            }
            kind => kind,
        };
        let outcome = (|| {
            self.check_live()?;
            let effective = self.admission.scoped(metadata.0, metadata.1)?;
            let heap = values.heap();
            let port = heap.restore_deferred(&metadata.2)?;
            let (kind, payload) = match kind {
                NativeNoticeKind::AstGrepLanguage { .. }
                | NativeNoticeKind::ReadDelimited { .. }
                | NativeNoticeKind::ReadDelimitedClosed
                | NativeNoticeKind::ReadAttachmentLookup { .. }
                | NativeNoticeKind::WriteBatchRequest { .. }
                | NativeNoticeKind::EditRepairClosed
                | NativeNoticeKind::EditRepairComplete { .. }
                | NativeNoticeKind::EditBlackboxSerialize { .. }
                | NativeNoticeKind::WriteProgress { .. } => {
                    unreachable!("handled acknowledged continuation")
                }
                NativeNoticeKind::Partial(output) => (0.0, output_graph(values, output)?),
                NativeNoticeKind::Complete(Ok(output)) => {
                    // Scope/parent/Run and notice sequence were checked above.
                    // Preserve the original delimiter aggregate/reason first.
                    let exact = self.effects.take_read_publication(notice.id);
                    match self.take_delimited_read_result(notice.id)? {
                        Some(Ok(edge)) => (1.0, heap.restore_deferred(&edge)?),
                        Some(Err(error)) => match error.restore(values)? {
                            ActualProviderError::Value(ValueError::Thrown(reason)) => (2.0, reason),
                            error => return Err(error),
                        },
                        None => match exact {
                            Some(ReadPublication::Output(output)) => (1.0, heap.import_strict(output)?),
                            Some(ReadPublication::TypeError(_)) => return Err(ActualProviderError::Protocol("READ failure completed with a successful worker token")),
                            None => (1.0, output_graph(values, output)?),
                        },
                    }
                },
                NativeNoticeKind::Complete(Err(fault)) => {
                    // Discard exact native data even on hard settlement faults.
                    let exact = self.effects.take_read_publication(notice.id);
                    if matches!(
                        fault,
                        NativeToolFault::JoinCancelled
                            | NativeToolFault::JoinPanicked
                            | NativeToolFault::Panicked(_)
                            | NativeToolFault::SequenceExhausted
                            | NativeToolFault::Closed
                    ) {
                        return Err(ActualProviderError::Callback(fault.message().to_owned()));
                    }
                    let error = if let Some(ReadPublication::TypeError(message)) = exact {
                        // The parser's loose optional-field TypeError remains
                        // on this original READ scope instead of a ToolError.
                        self.native_type_error(values, &message, &effective)?
                    } else {
                        let constructor = heap.restore_deferred(&self.error_constructor)?;
                        heap.call(&constructor, &heap.undefined()?, &[heap.string(fault.message().into())?], &effective)?
                    };
                    (2.0, error)
                }
            };
            heap.call(&port, &port, &[heap.number(kind)?, payload], &effective)?;
            Ok(())
        })();
        if let Err(error) = outcome {
            self.fail(error.clone());
            return Err(self.check_live().err().unwrap_or(error));
        }
        Ok(NativeHostDrain::RanJob)
    }
    pub(super) fn finish_graph(&self, id: HostEffectId) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).jobs.remove(&id);
        self.wake().notify_waiters();
    }
}
impl Drop for NativeBuiltinRun {
    fn drop(&mut self) {
        self.revoke();
    }
}

pub(super) struct ParentRegistration {
    pub admission: NativePromiseAdmission,
    pub id: NativeParentId,
}
impl Drop for ParentRegistration {
    fn drop(&mut self) {
        let _ = self.admission.release_parent_registration(self.id);
    }
}

pub(super) struct LegacyEffect {
    pub future: NativeEffectFuture,
    pub partials: Arc<Mutex<VecDeque<ToolOutput>>>,
    pub wake: Arc<Notify>,
}
impl LegacyEffect {
    pub async fn settle(mut self, runtime: &InvocationRuntime) -> Result<ValueHandle, ActualProviderError> {
        loop {
            let wake = self.wake.clone();
            let notified = wake.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let updates = self.partials.lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect::<Vec<_>>();
            for update in updates {
                runtime.emit(update, runtime);
            }
            tokio::select! {
                output = &mut self.future => {
                    let updates = self.partials.lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect::<Vec<_>>();
                    for update in updates { runtime.emit(update, runtime); }
                    if output.is_ok() && runtime.has_bash_preflight() {
                        self = runtime.resume_bash_legacy()?;
                        continue;
                    }
                    if output.is_ok() && runtime.has_ast_scope() {
                        self = runtime.resume_ast_legacy()?;
                        continue;
                    }
                    if output.is_err() { runtime.close_ast_scope(runtime); }
                    return output.map_err(|fault| ActualProviderError::Callback(fault.message().to_owned())).and_then(|output| {
                        let value = output_graph(&runtime.request.values, output)?;
                        runtime.finish_ast_output(&value,runtime)?;
                        runtime.finish_bash_legacy(&value)?;
                        Ok(value)
                    });
                }
                _ = notified => {}
            }
        }
    }
}

#[cfg(test)]
mod read_attachment_tests {
    use super::*;
    #[tokio::test]
    async fn read_attachment_lookup_cancel_without_graph_drain_settles_same_owner(){
        let effects=Arc::new(HostEffectSettlementOwner::new());let reservation=effects.reserve().unwrap();let publisher=reservation.publisher();let cancel=CancellationToken::new();
        reservation.enter(cancel.clone(),Box::pin(async move{publisher.lookup_read_attachment("attachment://1".into()).await?;Ok(ToolOutput::text("unexpected"))})).unwrap();
        // Observe that the real lookup notice is queued before cancellation.
        tokio::time::timeout(std::time::Duration::from_secs(2),async{while !effects.has_ready(){tokio::task::yield_now().await}}).await.unwrap();
        effects.cancel();effects.seal();
        let receipts=tokio::time::timeout(std::time::Duration::from_secs(2),effects.settle()).await.unwrap();
        assert_eq!(receipts.len(),1);assert!(receipts[0].failed());assert!(receipts[0].uncertain());
        assert_eq!(effects.settle().await[0].summary(),receipts[0].summary());
    }
}
