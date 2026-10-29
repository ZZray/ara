//! Original for-await import. Next/return Promises use the existing driver;
//! original event handles never cross a typed or serialized event adapter.
use super::*;
use ara_rpc::value_heap::{ValueKind, ValueRuntime};
pub struct NativeStreamInput {
    factory: Arc<GraphValueFactory>,
    bindings: Arc<dyn crate::retry::RetryBindings>,
    original: ValueHandle,
    iterator: Option<ValueHandle>,
    next: Option<ValueHandle>,
    synchronous: bool,
    done: bool,
}
impl NativeStreamInput {
    pub fn prepare(
        factory: Arc<GraphValueFactory>,
        bindings: Arc<dyn crate::retry::RetryBindings>,
        original: ValueHandle,
    ) -> Result<Self> {
        bindings.check_invocation(&factory)?;
        factory.heap().adopt(&original)?;
        Ok(Self {
            factory,
            bindings,
            original,
            iterator: None,
            next: None,
            synchronous: false,
            done: false,
        })
    }
    fn initialize(&mut self) -> Result<()> {
        if self.iterator.is_some() {
            return Ok(());
        }
        let (iterator, next, synchronous) =
            crate::retry::execute(&self.factory, self.bindings.as_ref(), |runtime| {
                let heap = self.factory.heap();
                let asynchronous =
                    heap.get(&self.original, &self.bindings.async_iterator_key(), runtime)?;
                let synchronous = matches!(
                    heap.kind(&asynchronous)?,
                    ValueKind::Undefined | ValueKind::Null
                );
                let method = if synchronous {
                    heap.get(&self.original, &heap.iterator_key(), runtime)?
                } else {
                    asynchronous
                };
                let iterator = heap.call(&method, &self.original, &[], runtime)?;
                require_object(heap, &iterator)?;
                let next = heap.get(&iterator, &"next".into(), runtime)?;
                Ok((iterator, next, synchronous))
            })?;
        self.iterator = Some(iterator);
        self.next = Some(next);
        self.synchronous = synchronous;
        Ok(())
    }
    pub async fn next(&mut self) -> Result<Option<ValueHandle>> {
        let result = self.next_inner().await;
        // for-await does not invoke return() for an abrupt IteratorNext/value
        // retrieval; IteratorClose applies to body abruptions and early exit.
        if result.is_err() {
            self.done = true;
        }
        result
    }
    async fn next_inner(&mut self) -> Result<Option<ValueHandle>> {
        if self.done {
            return Ok(None);
        }
        self.initialize()?;
        let value = crate::retry::execute(&self.factory, self.bindings.as_ref(), |runtime| {
            self.factory.heap().call(
                self.next.as_ref().ok_or(ValueError::InvalidNode)?,
                self.iterator.as_ref().ok_or(ValueError::InvalidNode)?,
                &[],
                runtime,
            )
        })?;
        let result = if self.synchronous {
            value
        } else {
            self.bindings
                .await_value(self.factory.clone(), value)
                .await?
        };
        let (done, value) =
            crate::retry::execute(&self.factory, self.bindings.as_ref(), |runtime| {
                let heap = self.factory.heap();
                require_object(heap, &result)?;
                let done = heap.truthy(&heap.get(&result, &"done".into(), runtime)?)?;
                let value = if done && !self.synchronous {
                    heap.undefined()?
                } else {
                    heap.get(&result, &"value".into(), runtime)?
                };
                Ok((done, value))
            })?;
        let value = if self.synchronous {
            match self.bindings.await_value(self.factory.clone(), value).await {
                Ok(value) => value,
                Err(primary) => {
                    // AsyncFromSync's rejected yielded value closes the sync
                    // iterator only when it had not already reported done.
                    // Its abrupt close invokes return synchronously; secondary
                    // Get/Call/type errors cannot replace this original reason.
                    if !done {
                        self.done = true;
                        let closed = crate::retry::execute(
                            &self.factory,
                            self.bindings.as_ref(),
                            |runtime| {
                                let heap = self.factory.heap();
                                let iterator =
                                    self.iterator.as_ref().ok_or(ValueError::InvalidNode)?;
                                let method = heap.get(iterator, &"return".into(), runtime)?;
                                if matches!(
                                    heap.kind(&method)?,
                                    ValueKind::Undefined | ValueKind::Null
                                ) {
                                    return Ok(());
                                }
                                let result = heap.call(&method, iterator, &[], runtime)?;
                                require_object(heap, &result)
                            },
                        );
                        if let Err(close) = closed {
                            if !close.is_native_exception() {
                                self.bindings.receipt().record(close);
                            }
                        }
                    }
                    return Err(primary);
                }
            }
        } else {
            value
        };
        if done {
            self.done = true;
            Ok(None)
        } else {
            Ok(Some(value))
        }
    }
    pub async fn close(&mut self) -> Result<()> {
        if self.done {
            return Ok(());
        }
        self.done = true;
        let Some(iterator) = self.iterator.clone() else {
            return Ok(());
        };
        let called = crate::retry::execute(&self.factory, self.bindings.as_ref(), |runtime| {
            let heap = self.factory.heap();
            let method = heap.get(&iterator, &"return".into(), runtime)?;
            if matches!(heap.kind(&method)?, ValueKind::Undefined | ValueKind::Null) {
                return Ok(None);
            }
            Ok(Some(heap.call(&method, &iterator, &[], runtime)?))
        })?;
        if let Some(value) = called {
            if self.synchronous {
                let value =
                    crate::retry::execute(&self.factory, self.bindings.as_ref(), |runtime| {
                        let heap = self.factory.heap();
                        require_object(heap, &value)?;
                        heap.get(&value, &"done".into(), runtime)?;
                        heap.get(&value, &"value".into(), runtime)
                    })?;
                self.bindings
                    .await_value(self.factory.clone(), value)
                    .await?;
            } else {
                let value = self
                    .bindings
                    .await_value(self.factory.clone(), value)
                    .await?;
                require_object(self.factory.heap(), &value)?;
            }
        }
        Ok(())
    }
}
fn require_object(heap: &Arc<ValueHeap>, value: &ValueHandle) -> Result<()> {
    if matches!(
        heap.kind(value)?,
        ValueKind::Object | ValueKind::Array | ValueKind::Function
    ) {
        Ok(())
    } else {
        Err(ValueError::TypeError(
            "Iterator result is not an object".into(),
        ))
    }
}
