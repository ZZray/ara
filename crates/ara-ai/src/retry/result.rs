//! Explicit original stream result import, fixed OMP596f2da event-stream.ts.
//! Event delivery, original result settlement and resource receipts are three
//! separate observations. Closing an event channel never infers a result.

use crate::{
    ActualAssistantEvent, ActualAssistantStream, ActualStreamFailureReceipt,
    event::{ActualStreamSettlementGuard, EventSink},
    graph_values::GraphValueFactory,
};
use ara_rpc::value_heap::{ValueError, ValueHandle, ValueHeap};
use futures::future::BoxFuture;
use std::sync::{Arc, Mutex, Weak};
use tokio::sync::Notify;
mod native;
pub use native::NativeStreamInput;
#[derive(Clone)]
pub enum CompletionEvent {
    Actual(ActualAssistantEvent),
    Original(ValueHandle),
}
pub trait CompletionErrorClassifier: Send + Sync {
    fn classify(&self, root: &ValueHandle) -> Result<()>;
    fn is_error(&self, root: &ValueHandle) -> Result<bool>;
    fn original_is_complete(&self, event: &ValueHandle) -> Result<bool>;
    fn original_result(&self, event: &ValueHandle) -> Result<ValueHandle>;
}

type Result<T> = std::result::Result<T, ValueError>;

/// This is the Rust language adapter for the source inner.result() method.
/// A native Host implementation must invoke its original method/receiver. A
/// Rust producer implementation publishes its actual end/fail result directly.
pub trait OriginalStreamResult: Send + Sync {
    fn result(&self) -> BoxFuture<'static, Result<ValueHandle>>;
}

/// Imported original stream object. Its result member is read only at the
/// source fallback position, and is called with the original receiver. The
/// existing issuing Run adopts the actual returned Promise or thenable.
pub struct NativeOriginalStreamResult {
    factory: Weak<GraphValueFactory>,
    bindings: Weak<dyn super::RetryBindings>,
    stream: ValueHandle,
}
impl NativeOriginalStreamResult {
    pub fn prepare(
        factory: &Arc<GraphValueFactory>,
        bindings: &Arc<dyn super::RetryBindings>,
        original_stream: ValueHandle,
    ) -> Result<Self> {
        factory.heap().adopt(&original_stream)?;
        Ok(Self {
            factory: Arc::downgrade(factory),
            bindings: Arc::downgrade(bindings),
            stream: original_stream,
        })
    }
}
impl OriginalStreamResult for NativeOriginalStreamResult {
    fn result(&self) -> BoxFuture<'static, Result<ValueHandle>> {
        let prepared = (|| {
            let factory = self
                .factory
                .upgrade()
                .ok_or(ValueError::UnsupportedOperation(
                    "Original stream result Session factory expired",
                ))?;
            let bindings = self
                .bindings
                .upgrade()
                .ok_or(ValueError::UnsupportedOperation(
                    "Original stream result calling Run expired",
                ))?;
            let value = super::execute(&factory, bindings.as_ref(), |runtime| {
                let method = factory
                    .heap()
                    .get(&self.stream, &"result".into(), runtime)?;
                factory.heap().call(&method, &self.stream, &[], runtime)
            })?;
            Ok::<_, ValueError>(bindings.await_value(factory, value))
        })();
        Box::pin(async move { prepared?.await })
    }
}

#[derive(Clone)]
pub struct OriginalResultOwner {
    heap_token: u64,
    state: Arc<Mutex<Option<Result<ValueHandle>>>>,
    wake: Arc<Notify>,
}
impl OriginalResultOwner {
    pub fn prepare(factory: &Arc<GraphValueFactory>) -> Self {
        Self {
            heap_token: factory.heap().heap_token(),
            state: Arc::new(Mutex::new(None)),
            wake: Arc::new(Notify::new()),
        }
    }
    fn check_value(&self, value: &ValueHandle) -> Result<()> {
        if value.heap().heap_token() != self.heap_token {
            return Err(ValueError::ForeignHeap);
        }
        Ok(())
    }
    /// First settlement wins. The same original root remains observable by all
    /// result waiters; no typed message or default usage is manufactured.
    pub fn resolve_original(&self, value: ValueHandle) -> Result<bool> {
        self.check_value(&value)?;
        self.publish(Ok(value))
    }
    pub fn reject_original(&self, error: ValueError) -> Result<bool> {
        if let ValueError::Thrown(value) = &error {
            self.check_value(value)?;
        }
        self.publish(Err(error))
    }
    fn publish(&self, result: Result<ValueHandle>) -> Result<bool> {
        let mut state = self.state.lock().map_err(|_| {
            ValueError::UnsupportedOperation("Original stream result owner is poisoned")
        })?;
        if state.is_some() {
            return Ok(false);
        }
        *state = Some(result);
        drop(state);
        self.wake.notify_waiters();
        Ok(true)
    }
    pub fn snapshot(&self) -> Result<Option<Result<ValueHandle>>> {
        Ok(self
            .state
            .lock()
            .map_err(|_| {
                ValueError::UnsupportedOperation("Original stream result owner is poisoned")
            })?
            .clone())
    }
    pub fn is_settled(&self) -> Result<bool> {
        Ok(self.snapshot()?.is_some())
    }
}
impl OriginalStreamResult for OriginalResultOwner {
    fn result(&self) -> BoxFuture<'static, Result<ValueHandle>> {
        let owner = self.clone();
        Box::pin(async move {
            loop {
                let notified = owner.wake.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if let Some(result) = owner.snapshot()? {
                    return result;
                }
                notified.await;
            }
        })
    }
}

/// Complete source stream view. Its result import is independent of event EOF
/// and of failure-receipt resource sealing. Buffered events drain before fail.
pub struct ActualCompletionStream {
    pub events: ActualAssistantStream,
    pub original_result: Arc<dyn OriginalStreamResult>,
    iteration_failure: Arc<Mutex<Option<ValueError>>>,
    native_events: tokio::sync::mpsc::UnboundedReceiver<ValueHandle>,
    native_input: Option<NativeStreamInput>,
}
impl ActualCompletionStream {
    pub fn channel(
        factory: &Arc<GraphValueFactory>,
    ) -> (ActualCompletionProducer, Self, ActualStreamFailureReceipt) {
        let (events, stream, receipt) = ActualAssistantStream::source_channel();
        let (native_events, native_receiver) = tokio::sync::mpsc::unbounded_channel();
        let result = OriginalResultOwner::prepare(factory);
        let iteration_failure = Arc::new(Mutex::new(None));
        let producer = ActualCompletionProducer {
            events: Some(events),
            result: result.clone(),
            done: false,
            iteration_failure: iteration_failure.clone(),
            receipt: receipt.clone(),
            native_events: Some(native_events),
            classifier: None,
            _settlement: receipt.owner_settlement_guard(),
        };
        (
            producer,
            Self {
                events: stream,
                original_result: Arc::new(result),
                iteration_failure,
                native_events: native_receiver,
                native_input: None,
            },
            receipt,
        )
    }
    pub fn result(&self) -> BoxFuture<'static, Result<ValueHandle>> {
        self.original_result.result()
    }
    pub fn from_native(
        factory: &Arc<GraphValueFactory>,
        bindings: &Arc<dyn super::RetryBindings>,
        original: ValueHandle,
    ) -> Result<Self> {
        let (producer, mut stream, _) = Self::channel(factory);
        drop(producer);
        stream.original_result = Arc::new(NativeOriginalStreamResult::prepare(
            factory,
            bindings,
            original.clone(),
        )?);
        stream.native_input = Some(NativeStreamInput::prepare(
            factory.clone(),
            bindings.clone(),
            original,
        )?);
        Ok(stream)
    }
    pub async fn recv_event(&mut self) -> Result<Option<CompletionEvent>> {
        if let Some(input) = &mut self.native_input {
            return input
                .next()
                .await
                .map(|event| event.map(CompletionEvent::Original));
        }
        self.recv()
            .await
            .map(|event| event.map(CompletionEvent::Actual))
    }
    pub async fn close_iteration(&mut self) -> Result<()> {
        if let Some(input) = &mut self.native_input {
            input.close().await?;
        }
        Ok(())
    }
    /// Original event roots stay intact; this native lane never builds a typed
    /// event or exports its custom metadata/getters/unknown type through JSON.
    pub async fn recv_original_event(&mut self) -> Result<Option<ValueHandle>> {
        if let Some(event) = self.native_events.recv().await {
            return Ok(Some(event));
        }
        if let Some(error) = self
            .iteration_failure
            .lock()
            .map_err(|_| ValueError::PoisonedHeap)?
            .clone()
        {
            return Err(error);
        }
        Ok(None)
    }
    pub async fn recv(&mut self) -> Result<Option<ActualAssistantEvent>> {
        if let Some(event) = self.events.recv().await {
            return Ok(Some(event));
        }
        // Normal source iterator end reads no result member and awaits no
        // result Promise. Only fail(err) throws after the queue has drained.
        // Retry's sole EOF fallback separately performs inner.result().
        if let Some(error) = self
            .iteration_failure
            .lock()
            .map_err(|_| {
                ValueError::UnsupportedOperation(
                    "Original stream iterator failure owner is poisoned",
                )
            })?
            .clone()
        {
            return Err(error);
        }
        Ok(None)
    }
}

/// Producer-owned original result publishing. Source end(root) can settle with
/// no terminal event. Source end() supplies its original native envelope error
/// through end_missing_result; it is never silently converted to a success.
pub struct ActualCompletionProducer {
    events: Option<EventSink<ActualAssistantEvent>>,
    result: OriginalResultOwner,
    done: bool,
    iteration_failure: Arc<Mutex<Option<ValueError>>>,
    receipt: ActualStreamFailureReceipt,
    _settlement: ActualStreamSettlementGuard,
    native_events: Option<tokio::sync::mpsc::UnboundedSender<ValueHandle>>,
    classifier: Option<Arc<dyn CompletionErrorClassifier>>,
}
impl ActualCompletionProducer {
    pub fn set_error_classifier(&mut self, classifier: Arc<dyn CompletionErrorClassifier>) {
        self.classifier = Some(classifier);
    }
    pub async fn push_event(&mut self, event: CompletionEvent) -> Result<bool> {
        match event {
            CompletionEvent::Actual(event) => self.push(event).await,
            CompletionEvent::Original(event) => self.push_original_event(event),
        }
    }
    pub fn push_original_event(&mut self, event: ValueHandle) -> Result<bool> {
        if self.done {
            return Ok(false);
        }
        let classifier = self
            .classifier
            .as_ref()
            .ok_or(ValueError::UnsupportedOperation(
                "Original event stream classifier is not installed",
            ))?;
        if classifier.original_is_complete(&event)? {
            self.done = true;
            let root = classifier.original_result(&event)?;
            self.result.resolve_original(root)?;
        }
        let sent = self
            .native_events
            .as_ref()
            .is_some_and(|sink| sink.send(event).is_ok());
        if self.done {
            self.events.take();
            self.native_events.take();
        }
        Ok(sent)
    }
    pub fn is_done(&self) -> bool {
        self.done
    }
    pub async fn closed(&self) {
        if let Some(events) = &self.events {
            events.closed().await;
        }
    }
    pub fn result_owner(&self) -> OriginalResultOwner {
        self.result.clone()
    }
    pub async fn push(&mut self, event: ActualAssistantEvent) -> Result<bool> {
        if self.done {
            return Ok(false);
        }
        let terminal = event.is_terminal();
        if let crate::AssistantMessageEvent::Error { error, .. } = &event {
            if let Some(classifier) = &self.classifier {
                if classifier.is_error(error)? {
                    classifier.classify(error)?;
                }
            }
        }
        if terminal {
            self.done = true;
            self.result.resolve_original(event.partial().clone())?;
            self.receipt.retain_terminal(&event);
        }
        let sent = match &self.events {
            Some(events) => events.push(event).await,
            None => false,
        };
        if terminal {
            self.events.take();
            self.native_events.take();
        }
        Ok(sent)
    }
    pub fn end_original(&mut self, root: ValueHandle) -> Result<()> {
        self.done = true;
        if let Some(classifier) = &self.classifier {
            if classifier.is_error(&root)? {
                classifier.classify(&root)?;
            }
        }
        self.result.resolve_original(root)?;
        self.events.take();
        self.native_events.take();
        Ok(())
    }
    pub fn end_missing_result(&mut self, original_envelope_error: ValueError) -> Result<()> {
        if self.done {
            return Ok(());
        }
        self.done = true;
        // Source end() rejects only result(), while iteration ends normally.
        if self
            .result
            .reject_original(original_envelope_error.clone())?
        {
            self.receipt.record(original_envelope_error);
        }
        self.events.take();
        self.native_events.take();
        Ok(())
    }
    pub fn fail_original(&mut self, error: ValueError) -> Result<()> {
        if self.done {
            return Ok(());
        }
        self.done = true;
        *self.iteration_failure.lock().map_err(|_| {
            ValueError::UnsupportedOperation("Original stream iterator failure owner is poisoned")
        })? = Some(error.clone());
        if self.result.reject_original(error.clone())? {
            self.receipt.record(error);
        }
        self.events.take();
        self.native_events.take();
        Ok(())
    }
}
