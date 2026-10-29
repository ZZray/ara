//! Actual arm of fixed OMP agent-loop.ts:1906-1951 (MIT).
//! Source selection and arguments stay in the original Session realm.
use super::*;
use crate::tool::ActualToolBinding;
use crate::{ActualProviderError, SessionValues};
use ara_ai::ActualAssistantEvent;
use ara_rpc::value_heap::{PropertyKey, ValueHandle, ValueKind, ValueSnapshot};

fn string(values: &SessionValues, value: &ValueHandle) -> Result<String, ToolError> {
    values.check_handle(value).map_err(|error| ToolError(error.to_string()))?;
    let ValueSnapshot::String(text) = values.heap().snapshot(value).map_err(|error| ToolError(error.to_string()))?
    else {
        return Err(ToolError("argument preview scalar is not a string".into()));
    };
    text.to_utf8().map_err(|_| ToolError("argument preview UTF16 boundary".into()))
}

/// Native indexed Get uses ToPropertyKey; the retained Map key itself never
/// undergoes coercion, so object keys and negative/string indices keep identity.
fn indexed_get(
    values: &SessionValues,
    content: &ValueHandle,
    index: &ValueHandle,
) -> Result<ValueHandle, ActualProviderError> {
    let heap = values.heap();
    let primitive = heap.to_primitive(index, true, values.runtime())?;
    let key = match heap.snapshot(&primitive)? {
        ValueSnapshot::Symbol { key, .. } => PropertyKey::Symbol(key),
        _ => heap.to_string(&primitive, values.runtime())?.into(),
    };
    Ok(heap.get(content, &key, values.runtime())?)
}

impl ArgStreams {
    fn actual_position(
        &self,
        index: &ValueHandle,
        values: &SessionValues,
    ) -> Result<Option<usize>, ActualProviderError> {
        for (position, (key, _)) in self.actual_entries.iter().enumerate() {
            // SameValueZero: Object.is already covers identities and NaN; Map
            // also considers signed zeros equal, without any callback/coercion.
            if values.heap().object_is(key, index)?
                || matches!(
                    (values.heap().snapshot(key)?, values.heap().snapshot(index)?),
                    (ValueSnapshot::Number(left), ValueSnapshot::Number(right)) if left == 0.0 && right == 0.0
                )
            {
                return Ok(Some(position));
            }
        }
        Ok(None)
    }

    pub(crate) fn on_actual_event(
        &mut self,
        event: &ActualAssistantEvent,
        values: &SessionValues,
        resolve: impl FnOnce(&ValueHandle) -> Result<Option<ActualToolBinding>, ActualProviderError>,
    ) -> Result<(), ActualProviderError> {
        let heap = values.heap();
        match event {
            AssistantMessageEvent::ToolcallStart { content_index, partial } => {
                let content = heap.get(partial, &"content".into(), values.runtime())?;
                let call = indexed_get(values, &content, content_index)?;
                if matches!(heap.kind(&call)?, ValueKind::Undefined | ValueKind::Null) {
                    return Ok(());
                }
                let kind = heap.get(&call, &"type".into(), values.runtime())?;
                if !heap.object_is(&kind, &heap.string("toolCall".into())?)? {
                    return Ok(());
                }
                let Some(binding) = resolve(&call)? else { return Ok(()) };
                let init = consumer("actual", "open metadata", || {
                    let get = |key: &str| {
                        heap.get(&call, &key.into(), values.runtime()).map_err(|error| ToolError(error.to_string()))
                    };
                    let id = string(values, &get("id")?)?;
                    let name = string(values, &get("name")?)?;
                    let wire = get("customWireName")?;
                    // Fixed rawInput uses !== undefined, including an explicit
                    // empty custom wire name. Null is not a string binding.
                    let wire = match heap.kind(&wire).map_err(|error| ToolError(error.to_string()))? {
                        ValueKind::Undefined => None,
                        _ => Some(string(values, &wire)?),
                    };
                    let updates = Arc::downgrade(&self.updates);
                    let tool_call_id = id.clone();
                    let tool_name = name.clone();
                    let emit = Arc::new(move |update: Value| {
                        let Some(updates) = updates.upgrade() else { return };
                        let mut updates = updates.lock().unwrap_or_else(|error| error.into_inner());
                        if updates.open {
                            updates.pending.push_back(AgentEvent::ToolStreamUpdate {
                                tool_call_id: tool_call_id.clone(),
                                tool_name: tool_name.clone(),
                                update,
                            });
                            updates.wake.notify_one();
                        }
                    });
                    Ok(ToolArgStreamInit { tool_call_id: id, tool_name: name, custom_wire_name: wire, emit })
                });
                if let Some(init) = init {
                    let id = init.tool_call_id.clone();
                    if let Some(Some(stream)) = consumer(&id, "open", || binding.capability.open_arg_stream(init)) {
                        let entry = Entry { id, stream };
                        if let Some(position) = self.actual_position(content_index, values)? {
                            self.actual_entries[position] = (content_index.clone(), entry);
                        } else {
                            self.actual_entries.push((content_index.clone(), entry));
                        }
                    }
                }
            }
            AssistantMessageEvent::ToolcallDelta { content_index, delta, .. } => {
                if let Some(position) = self.actual_position(content_index, values)? {
                    let entry = &mut self.actual_entries[position].1;
                    let _ = consumer(&entry.id, "push", || entry.stream.push(&string(values, delta)?));
                }
            }
            AssistantMessageEvent::ToolcallEnd { content_index, tool_call, .. } => {
                if let Some(position) = self.actual_position(content_index, values)? {
                    let (_, mut entry) = self.actual_entries.remove(position);
                    let _ = consumer(&entry.id, "end", || {
                        let args = heap
                            .get(tool_call, &"arguments".into(), values.runtime())
                            .map_err(|error| ToolError(error.to_string()))?;
                        entry.stream.end_actual(&args, values)
                    });
                }
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ara_rpc::value_heap::{NoRuntime, ValueHeap};
    use async_trait::async_trait;
    use serde_json::json;

    struct PreviewTool {
        args: ValueHandle,
        calls: Arc<Mutex<Vec<&'static str>>>,
    }
    struct Receiver {
        args: ValueHandle,
        calls: Arc<Mutex<Vec<&'static str>>>,
        init: ToolArgStreamInit,
    }
    impl ToolArgStream for Receiver {
        fn push(&mut self, delta: &str) -> Result<(), ToolError> {
            assert_eq!(delta, "original delta");
            self.calls.lock().unwrap().push("push");
            (self.init.emit)(json!({"phase":"push"}));
            Ok(())
        }
        fn end(&mut self, _: &ara_ai::JsonObject) -> Result<(), ToolError> {
            panic!("actual arguments must not enter a typed projection")
        }
        fn end_actual(&mut self, args: &ValueHandle, _: &SessionValues) -> Result<(), ToolError> {
            assert!(args.same_identity(&self.args));
            self.calls.lock().unwrap().push("end");
            (self.init.emit)(json!({"phase":"end"}));
            Ok(())
        }
        fn cancel(&mut self) -> Result<(), ToolError> {
            self.calls.lock().unwrap().push("cancel");
            Ok(())
        }
    }
    #[async_trait]
    impl AgentTool for PreviewTool {
        fn definition(&self) -> ara_ai::Tool {
            panic!("actual source binding must not read typed definitions")
        }
        fn open_arg_stream(&self, init: ToolArgStreamInit) -> Result<Option<Box<dyn ToolArgStream>>, ToolError> {
            assert_eq!(init.tool_call_id, "original-id");
            assert_eq!(init.tool_name, "edit");
            assert_eq!(init.custom_wire_name.as_deref(), Some("apply_patch"));
            self.calls.lock().unwrap().push("open");
            Ok(Some(Box::new(Receiver { args: self.args.clone(), calls: self.calls.clone(), init })))
        }
        async fn execute(
            &self,
            _: &str,
            _: ara_ai::JsonObject,
            _: tokio_util::sync::CancellationToken,
            _: crate::tool::UpdateFn,
        ) -> Result<crate::tool::ToolOutput, ToolError> {
            panic!("preview does not confer tool effect authority")
        }
    }

    #[tokio::test]
    async fn original_actual_preview_identity_and_attempt_cleanup_group() {
        // Existing neutral heap runtime, not a second execution Runtime. This
        // fixture covers only argument transport; real Native Host gates remain.
        let heap = ValueHeap::new().unwrap();
        let values = SessionValues::new(heap.clone(), Arc::new(NoRuntime)).unwrap();
        let args = heap.object().unwrap();
        let call = heap.object().unwrap();
        for (key, value) in [
            ("id", heap.string("original-id".into()).unwrap()),
            ("name", heap.string("edit".into()).unwrap()),
            ("type", heap.string("toolCall".into()).unwrap()),
            ("customWireName", heap.string("apply_patch".into()).unwrap()),
            ("arguments", args.clone()),
        ] {
            heap.define_own_data(&call, key.into(), value, &NoRuntime).unwrap();
        }
        let partial = heap.object().unwrap();
        heap.define_own_data(&partial, "content".into(), heap.array(vec![call.clone()]).unwrap(), &NoRuntime).unwrap();
        let source = heap.object().unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let binding = ActualToolBinding {
            source,
            capability: Arc::new(PreviewTool { args: args.clone(), calls: calls.clone() }),
        };
        let start = AssistantMessageEvent::ToolcallStart {
            content_index: heap.number(-0.0).unwrap(),
            partial: partial.clone(),
        };
        let mut streams = ArgStreams::new();
        streams
            .on_actual_event(&start, &values, |selected| {
                assert!(selected.same_identity(&call));
                Ok(Some(binding.clone()))
            })
            .unwrap();
        streams
            .on_actual_event(
                &AssistantMessageEvent::ToolcallDelta {
                    content_index: heap.number(0.0).unwrap(),
                    delta: heap.string("original delta".into()).unwrap(),
                    partial: partial.clone(),
                },
                &values,
                |_| panic!("deltas do not resolve a tool again"),
            )
            .unwrap();
        streams
            .on_actual_event(
                &AssistantMessageEvent::ToolcallEnd {
                    content_index: heap.number(0.0).unwrap(),
                    tool_call: call.clone(),
                    partial: partial.clone(),
                },
                &values,
                |_| panic!("end uses the same receiver"),
            )
            .unwrap();
        let sink = crate::RecordingSink::default();
        streams.finish(&sink).await;
        assert_eq!(*calls.lock().unwrap(), vec!["open", "push", "end"]);
        let frames = sink.events.lock().await;
        assert_eq!(frames.len(), 2);
        assert!(
            frames.iter().all(|event| matches!(event, AgentEvent::ToolStreamUpdate { .. })),
            "preview cannot become a tool receipt"
        );
        drop(frames);
        // Error/EOF/cancel all enter finish; dropped attempts enter Drop. Only
        // unfinished receivers are cancelled, with original insertion order.
        let mut streams = ArgStreams::new();
        streams.on_actual_event(&start, &values, |_| Ok(Some(binding.clone()))).unwrap();
        streams.finish(&sink).await;
        assert_eq!(*calls.lock().unwrap(), vec!["open", "push", "end", "open", "cancel"]);
        let mut streams = ArgStreams::new();
        streams.on_actual_event(&start, &values, |_| Ok(Some(binding))).unwrap();
        drop(streams);
        assert_eq!(*calls.lock().unwrap(), vec!["open", "push", "end", "open", "cancel", "open", "cancel"]);
    }
}
