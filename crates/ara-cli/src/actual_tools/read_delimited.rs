//! Fixed OMP596f2da read.ts:767-813, original Graph delimiter aggregation.
//! MIT; Copyright (c) 2025 Mario Zechner; 2025-2026 Can Bölük;
//! 2026 Stencil Labs, Inc. See THIRD_PARTY.md.
//! The worker sends paths only. Each part runs the existing admitted Native
//! method, including its original execute Get/Call and post-READ continuation.
use super::*;
use ara_rpc::{
    WireString,
    value_heap::{ValueHeap, ValueKind},
};

pub(super) struct Dispatcher(pub(super) async_effect::NativeEffectPublisher);
#[async_trait]
impl ara_tools::read::ReadDelimitedDispatcher for Dispatcher {
    async fn read_parts(
        &self,
        parts: Vec<String>,
        _cancel: CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        self.0
            .read_delimited(parts)
            .await
            .map_err(|error| ToolError(error.message().to_owned()))?;
        // This is a worker completion token. The exact Graph aggregate/reason
        // is retained by the driver and substituted before kernel adoption.
        Ok(ToolOutput::text(""))
    }
}

pub(super) struct Bindings {
    pub environment: ValueHandle,
    pub error: ValueHandle,
    pub abort: ValueHandle,
    pub native_errors: [ValueHandle; 3],
}
impl Bindings {
    fn native(
        &self,
        heap: &Arc<ValueHeap>,
        error: ValueError,
        runtime: &dyn ValueRuntime,
    ) -> ValueError {
        let (index, message) = match error {
            ValueError::TypeError(message) => (0, message),
            ValueError::SyntaxError(message) => (1, message),
            ValueError::RangeError(message) => (2, message),
            other => return other,
        };
        match (|| {
            heap.call(
                &self.native_errors[index],
                &heap.undefined()?,
                &[heap.string(message.into())?],
                runtime,
            )
        })() {
            Ok(reason) => ValueError::Thrown(reason),
            Err(error) => error,
        }
    }
}
fn nullish(heap: &Arc<ValueHeap>, value: &ValueHandle) -> Result<bool, ValueError> {
    Ok(matches!(
        heap.kind(value)?,
        ValueKind::Undefined | ValueKind::Null
    ))
}
fn get(
    heap: &Arc<ValueHeap>,
    object: &ValueHandle,
    key: &str,
    r: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    heap.get(object, &key.into(), r)
}
fn push(
    heap: &Arc<ValueHeap>,
    array: &ValueHandle,
    value: ValueHandle,
    r: &dyn ValueRuntime,
) -> Result<(), ValueError> {
    let method = get(heap, array, "push", r)?;
    heap.call(&method, array, &[value], r)?;
    Ok(())
}
fn implicit_string(
    heap: &Arc<ValueHeap>,
    value: &ValueHandle,
    r: &dyn ValueRuntime,
) -> Result<WireString, ValueError> {
    let primitive = heap.to_primitive(value, true, r)?;
    if heap.kind(&primitive)? == ValueKind::Symbol {
        return Err(ValueError::TypeError(
            "Cannot convert a Symbol value to a string".into(),
        ));
    }
    heap.to_string(&primitive, r)
}
fn positive(
    heap: &Arc<ValueHeap>,
    value: &ValueHandle,
    r: &dyn ValueRuntime,
) -> Result<bool, ValueError> {
    let primitive = heap.to_primitive(value, false, r)?;
    if let ValueSnapshot::BigInt(value) = heap.snapshot(&primitive)? {
        return Ok(!value.as_str().starts_with('-') && value.as_str() != "0");
    }
    Ok(heap.to_number(&primitive, r)? > 0.0)
}
fn close_error(
    heap: &Arc<ValueHeap>,
    iterator: &mut ara_rpc::value_heap::IteratorRecord,
    error: ValueError,
    r: &dyn ValueRuntime,
) -> ValueError {
    match heap.iterator_close(iterator, r) {
        Ok(()) => error,
        Err(close) if error.is_native_exception() && close.is_native_exception() => error,
        Err(close) => ValueError::IteratorClose {
            original: Box::new(error),
            close: Box::new(close),
        },
    }
}
struct Aggregate {
    notes: ValueHandle,
    content: ValueHandle,
    display: ValueHandle,
    pending: ValueHandle,
}
impl Aggregate {
    fn new(heap: &Arc<ValueHeap>, parts: &[String]) -> Result<Self, ValueError> {
        let notice = heap.string(
            format!(
                "Note: interpreted as {} paths: {}",
                parts.len(),
                parts.join(", ")
            )
            .into(),
        )?;
        Ok(Self {
            notes: heap.array(vec![notice.clone()])?,
            content: heap.array(Vec::new())?,
            display: heap.array(Vec::new())?,
            pending: notice,
        })
    }
    fn append(
        &mut self,
        heap: &Arc<ValueHeap>,
        text: ValueHandle,
        r: &dyn ValueRuntime,
    ) -> Result<(), ValueError> {
        // Source tests pendingText.length > 0 (not truthiness of pendingText).
        self.pending = if positive(heap, &get(heap, &self.pending, "length", r)?, r)? {
            let mut units = implicit_string(heap, &self.pending, r)?.units().to_vec();
            units.extend("\n\n".encode_utf16());
            units.extend_from_slice(implicit_string(heap, &text, r)?.units());
            heap.string(WireString::from_units(units))?
        } else {
            text
        };
        Ok(())
    }
    fn flush(&mut self, heap: &Arc<ValueHeap>, r: &dyn ValueRuntime) -> Result<(), ValueError> {
        let length = get(heap, &self.pending, "length", r)?;
        if matches!(heap.snapshot(&length)?, ValueSnapshot::Number(length) if length == 0.0) {
            return Ok(());
        }
        // content.push Get precedes the object literal's operand evaluation.
        let push = get(heap, &self.content, "push", r)?;
        let block = heap.object()?;
        heap.define_own_data(&block, "type".into(), heap.string("text".into())?, r)?;
        heap.define_own_data(&block, "text".into(), self.pending.clone(), r)?;
        heap.call(&push, &self.content, &[block], r)?;
        self.pending = heap.string("".into())?;
        Ok(())
    }
    fn accept(
        &mut self,
        heap: &Arc<ValueHeap>,
        part: &str,
        result: &ValueHandle,
        r: &dyn ValueRuntime,
    ) -> Result<(), ValueError> {
        // Preserve source evaluation order: push method is evaluated before
        // result.details?.suffixResolution?.to and its nullish fallback.
        let display_push = get(heap, &self.display, "push", r)?;
        let details = get(heap, result, "details", r)?;
        let suffix = if nullish(heap, &details)? {
            heap.undefined()?
        } else {
            get(heap, &details, "suffixResolution", r)?
        };
        let target = if nullish(heap, &suffix)? {
            heap.undefined()?
        } else {
            get(heap, &suffix, "to", r)?
        };
        let target = if nullish(heap, &target)? {
            heap.string(part.into())?
        } else {
            target
        };
        heap.call(&display_push, &self.display, &[target], r)?;
        let content = get(heap, result, "content", r)?;
        let mut iterator = heap.get_iterator(&content, r)?;
        loop {
            let Some(block) = heap.iterator_next(&mut iterator, r)? else {
                break;
            };
            let result = (|| {
                let kind = get(heap, &block, "type", r)?;
                if matches!(heap.snapshot(&kind)?, ValueSnapshot::String(kind) if kind.equals_ascii("text"))
                {
                    self.append(heap, get(heap, &block, "text", r)?, r)?;
                } else {
                    self.flush(heap, r)?;
                    push(heap, &self.content, block, r)?;
                }
                Ok(())
            })();
            if let Err(error) = result {
                return Err(close_error(heap, &mut iterator, error, r));
            }
        }
        Ok(())
    }
    fn failed(
        &mut self,
        heap: &Arc<ValueHeap>,
        bindings: &Bindings,
        signal: &ValueHandle,
        part: &str,
        error: ValueError,
        r: &dyn ValueRuntime,
    ) -> Result<(), ValueError> {
        let error = bindings.native(heap, error, r);
        let ValueError::Thrown(reason) = error else {
            return Err(error);
        };
        if heap.instance_of(&reason, &bindings.abort, r)?
            || (!nullish(heap, signal)? && heap.truthy(&get(heap, signal, "aborted", r)?)?)
        {
            return Err(ValueError::Thrown(reason));
        }
        let message = if heap.instance_of(&reason, &bindings.error, r)? {
            get(heap, &reason, "message", r)?
        } else {
            let string = get(heap, &bindings.environment, "String", r)?;
            heap.call(&string, &heap.undefined()?, &[reason], r)?
        };
        let mut units = format!("Could not read {part}: ")
            .encode_utf16()
            .collect::<Vec<_>>();
        units.extend_from_slice(implicit_string(heap, &message, r)?.units());
        let note = heap.string(WireString::from_units(units.clone()))?;
        push(heap, &self.notes, note, r)?;
        push(heap, &self.display, heap.string(part.into())?, r)?;
        let mut bracketed = vec![u16::from(b'[')];
        bracketed.extend(units);
        bracketed.push(u16::from(b']'));
        self.append(heap, heap.string(WireString::from_units(bracketed))?, r)
    }
    fn done(
        mut self,
        heap: &Arc<ValueHeap>,
        r: &dyn ValueRuntime,
    ) -> Result<ValueHandle, ValueError> {
        self.flush(heap, r)?;
        let details = heap.object()?;
        heap.define_own_data(&details, "notes".into(), self.notes, r)?;
        heap.define_own_data(&details, "displayReadTargets".into(), self.display, r)?;
        let result = heap.object()?;
        heap.define_own_data(&result, "content".into(), self.content, r)?;
        heap.define_own_data(&result, "details".into(), details, r)?;
        Ok(result)
    }
}

pub(super) fn execute(
    ancestor: Arc<InvocationRuntime>,
    run: Arc<NativeBuiltinRun>,
    scope: crate::native_realm::context::NativeScope,
    parent: NativeParentId,
    parts: Vec<String>,
    bindings: Bindings,
) -> BoxFuture<'static, Result<ValueHandle, ActualProviderError>> {
    Box::pin(async move {
        let values = ancestor.request.values.clone();
        let heap = values.heap();
        let mut aggregate = Aggregate::new(heap, &parts)?;
        for part in parts {
            // Every iteration obtains this.execute through the sealed supplier.
            // The fresh params and context are original same-realm graph data.
            let child = {
                run.check_live()?;
                ancestor.request.check_execution_ready()?;
                let effective = run.admission.scoped(scope, parent)?;
                let args = heap.object()?;
                heap.define_own_data(
                    &args,
                    "path".into(),
                    heap.string(part.clone().into())?,
                    &effective,
                )?;
                let member = &ancestor.owner.members[ancestor.index];
                let binding = ActualToolBinding {
                    source: ancestor.request.receiver.clone(),
                    capability: member.capability.clone(),
                };
                NativeMethodRequest::read_delimited_part(
                    ancestor.clone(),
                    binding,
                    args,
                    heap.object()?,
                    &effective,
                )
            };
            let result = match child {
                Ok(request) => {
                    BuiltinInvoker {
                        owner: Arc::downgrade(&ancestor.owner),
                        index: ancestor.index,
                    }
                    .invoke_read_part(request)
                    .await
                }
                Err(error) => Err(error),
            };
            run.check_live()?;
            let effective = run.admission.scoped(scope, parent)?;
            let outcome = match result {
                Ok(result) => aggregate
                    .accept(heap, &part, &result, &effective)
                    .map_err(|error| {
                        ActualProviderError::Value(bindings.native(heap, error, &effective))
                    }),
                Err(error) => Err(error),
            };
            match outcome {
                Ok(()) => {}
                Err(ActualProviderError::Value(error)) => aggregate
                    .failed(
                        heap,
                        &bindings,
                        &ancestor.arguments[2],
                        &part,
                        error,
                        &effective,
                    )
                    .map_err(|error| {
                        ActualProviderError::Value(bindings.native(heap, error, &effective))
                    })?,
                Err(error) => return Err(error),
            }
        }
        let effective = run.admission.scoped(scope, parent)?;
        aggregate
            .done(heap, &effective)
            .map_err(|error| ActualProviderError::Value(bindings.native(heap, error, &effective)))
    })
}

#[cfg(test)]
mod tests;
