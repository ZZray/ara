//! CLI Host encoder for the existing native-body Session journal.
//! Fixed OMP 596f2da7101178214aa27a753529d15e6b7ad91d; MIT notices in THIRD_PARTY.md.
//! The live body and runtime lease remain canonical. JSON/Wire is a derived
//! observation or durable boundary, never a replacement execution transcript.

use std::{
    collections::HashSet,
    io,
    sync::{Arc, Mutex},
};

use ara_agent::{AgentMessage, SessionValues};
use ara_rpc::{
    WireString, WireValue,
    value_heap::{NodeId, PropertyKey, ValueError, ValueHandle, ValueHeap, ValueRuntime, ValueSnapshot},
};
use ara_session::{
    BlobStore, NativeJournalEncoder, NativeMessageSource,
    actual_persistence::{
        ActualPersistenceBlobs, ActualPersistenceModule, PersistenceBufferEncoding, buffer_from_wire_string,
    },
};
use serde_json::Value;

pub struct GraphJournalEncoder {
    values: Arc<SessionValues>,
    persistence: ActualPersistenceModule,
    native_call_depth: Option<usize>,
}

impl GraphJournalEncoder {
    /// None uses the iterative persistence traversal without an artificial
    /// depth cap. Some is an explicit Host frame budget, not a JS stack profile.
    pub fn new(values: Arc<SessionValues>, native_call_depth: Option<usize>) -> Result<Self, ValueError> {
        let persistence = ActualPersistenceModule::new(values.heap().clone())?;
        Ok(Self { values, persistence, native_call_depth })
    }

    pub fn values(&self) -> &Arc<SessionValues> {
        &self.values
    }

    /// Reopening imports only original durable data into this fresh realm.
    /// Same-realm sources retain the exact original handle and Host lease.
    pub fn message_from_source(
        self: &Arc<Self>,
        source: &NativeMessageSource,
        envelope: Option<Arc<Value>>,
    ) -> io::Result<AgentMessage> {
        let body = match source {
            NativeMessageSource::Live { body, .. } => {
                self.values.check_handle(body).map_err(value_error)?;
                body.clone()
            }
            NativeMessageSource::Persisted { .. } => {
                let current = source.bind_current(self.values.heap(), self.clone())?;
                current
                    .live_body()
                    .ok_or_else(|| invalid_observation("current realm binding has no live body"))?
                    .clone()
            }
        };
        AgentMessage::from_graph(body, envelope).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }
}

fn value_error(error: ValueError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

/// This adapter is local to one encode operation; it retains the original I/O
/// failure for the existing writer/rollback owner. Missing blob storage cannot
/// manufacture a reference. Other native Buffer carriers remain an explicit
/// Host integration gap until their current runtime binding is supplied.
struct JournalBlobs<'a> {
    store: Option<&'a BlobStore>,
    failure: Mutex<Option<io::Error>>,
}

impl ActualPersistenceBlobs for JournalBlobs<'_> {
    fn buffer_from(
        &self,
        heap: &Arc<ValueHeap>,
        data: &ValueHandle,
        encoding: PersistenceBufferEncoding,
        _runtime: &dyn ValueRuntime,
    ) -> Result<Vec<u8>, ValueError> {
        buffer_from_wire_string(heap, data, encoding)
    }

    fn put_sync(&self, bytes: &[u8], extension: Option<&WireString>) -> Result<WireString, ValueError> {
        let store =
            self.store.ok_or(ValueError::UnsupportedOperation("CLI native journal BlobStore is unavailable"))?;
        let extension = extension
            .map(WireString::to_utf8)
            .transpose()
            .map_err(|_| ValueError::UnsupportedOperation("CLI native BlobStore extension UTF16 boundary"))?;
        match store.put(bytes, extension.as_deref()) {
            Ok(result) => Ok(result.reference.into()),
            Err(error) => {
                *self.failure.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(error);
                Err(ValueError::UnsupportedOperation("CLI native BlobStore I/O failure with original Host receipt"))
            }
        }
    }
}

impl NativeJournalEncoder for GraphJournalEncoder {
    fn prepare_tool_result_edit(&self, body: &ValueHandle, before: &Value, after: &Value) -> io::Result<ValueHandle> {
        use ara_rpc::value_heap::PropertyDescriptor;
        self.values.check_handle(body).map_err(value_error)?;
        let (Some(previous), Some(next)) = (before.as_object(), after.as_object()) else {
            return Err(invalid_observation("native tool-result edit requires object observations"));
        };
        if before["role"] != "toolResult" || after["role"] != "toolResult" {
            return Err(invalid_observation("native live edit is limited to toolResult"));
        }
        let unchanged = |fields: &serde_json::Map<String, Value>| {
            Value::Object(
                fields
                    .iter()
                    .filter(|(key, _)| !["content", "prunedAt"].contains(&key.as_str()))
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            )
        };
        if !ara_session::native_wire_from_serde(&unchanged(previous))?
            .deep_equal(&ara_session::native_wire_from_serde(&unchanged(next))?)
        {
            return Err(invalid_observation("native live edit changed fields outside content/prunedAt"));
        }
        let heap = self.values.heap();
        let runtime = self.values.runtime();
        if !matches!(heap.snapshot(body).map_err(value_error)?, ValueSnapshot::Object) {
            return Err(invalid_observation("native live edit requires an ordinary object root"));
        }
        let prototype = heap.prototype(body, runtime).map_err(value_error)?;
        let candidate = heap.object_with_prototype(prototype.as_ref()).map_err(value_error)?;
        // Retain descriptors, Symbols, executable/opaque extensions and their
        // original graph edges without reading any copied property value.
        for key in heap.own_keys(body, runtime).map_err(value_error)? {
            let descriptor = heap
                .own_descriptor(body, &key, runtime)
                .map_err(value_error)?
                .ok_or_else(|| invalid_observation("native live edit descriptor disappeared"))?;
            heap.define_own(&candidate, key, descriptor, runtime).map_err(value_error)?;
        }
        if !heap.is_extensible(body, runtime).map_err(value_error)? {
            heap.set_extensible(&candidate, false, runtime).map_err(value_error)?;
        }
        for name in ["content", "prunedAt"] {
            if previous.get(name) == next.get(name) {
                continue;
            }
            let key = PropertyKey::from(name);
            match next.get(name) {
                Some(value) => {
                    let value = heap.import_serde(value).map_err(value_error)?;
                    let descriptor = match heap.own_descriptor(&candidate, &key, runtime).map_err(value_error)? {
                        Some(PropertyDescriptor::Data { writable, enumerable, configurable, .. }) => {
                            PropertyDescriptor::Data { value, writable, enumerable, configurable }
                        }
                        None => PropertyDescriptor::data(value),
                        Some(PropertyDescriptor::Accessor { .. }) => {
                            return Err(invalid_observation("native edited slot is an accessor"));
                        }
                    };
                    heap.define_own(&candidate, key, descriptor, runtime).map_err(value_error)?;
                }
                None => {
                    if !heap.delete_own(&candidate, &key, runtime).map_err(value_error)? {
                        return Err(invalid_observation("native edited slot cannot be deleted"));
                    }
                }
            }
        }
        Ok(candidate)
    }

    fn encode_entry(&self, envelope: &Value, body: &ValueHandle, blobs: Option<&BlobStore>) -> io::Result<WireString> {
        self.values.check_handle(body).map_err(value_error)?;
        let mut metadata = envelope
            .as_object()
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "native journal envelope must be an object"))?;
        // A checked legacy mirror is derived; only envelope metadata enters
        // this import. The original live body is linked without conversion.
        let custom = metadata.get("type").and_then(Value::as_str) == Some("custom_message");
        if custom {
            metadata.retain(|key, _| ["type", "id", "parentId", "timestamp"].contains(&key.as_str()));
        } else {
            metadata.remove("message");
        }
        let entry = self.values.heap().import_serde(&Value::Object(metadata)).map_err(value_error)?;
        let heap = self.values.heap();
        let runtime = self.values.runtime();
        if custom {
            for key in heap.own_enumerable_string_keys(body, runtime).map_err(value_error)? {
                if key.equals_ascii("role") || key.equals_ascii("timestamp") {
                    continue;
                }
                if ["type", "id", "parentId"].iter().any(|reserved| key.equals_ascii(reserved)) {
                    return Err(invalid_observation("native custom body collides with its journal envelope"));
                }
                let value = heap.get(body, &key.clone().into(), runtime).map_err(value_error)?;
                heap.define_own_data(&entry, key.into(), value, runtime).map_err(value_error)?;
            }
        } else {
            heap.define_own_data(&entry, "message".into(), body.clone(), runtime).map_err(value_error)?;
        }
        let blob_port = JournalBlobs { store: blobs, failure: Mutex::new(None) };
        let encoded = self.persistence.line_for(&entry, &blob_port, self.values.runtime(), self.native_call_depth);
        match encoded {
            Ok(line) => Ok(line),
            Err(error) => Err(blob_port
                .failure
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take()
                .unwrap_or_else(|| value_error(error))),
        }
    }

    fn observe_body(&self, body: &ValueHandle) -> io::Result<WireValue> {
        self.values.check_handle(body).map_err(value_error)?;
        // This is an explicit checked legacy bridge. It neither invokes a
        // serializer/toJSON nor applies persistence preprocessing. Native-only
        // domains fail rather than silently changing Number or UTF16 values.
        let legacy = self.values.heap().export_serde(body).map_err(value_error)?;
        ara_session::native_wire_from_serde(&legacy)
    }

    fn observe_compaction_message_body(&self, body: &ValueHandle) -> io::Result<WireValue> {
        self.values.check_handle(body).map_err(value_error)?;
        let role = self.values.heap().get(body, &"role".into(), self.values.runtime()).map_err(value_error)?;
        if matches!(self.values.heap().snapshot(&role).map_err(value_error)?,
            ValueSnapshot::String(role) if role.equals_ascii("assistant") || role.equals_ascii("toolResult"))
        {
            // The existing observer selects known model fields/blocks and
            // retains strict original arguments/payloads/details. Optional
            // Undefined metadata stays on the canonical tool-result root.
            let observed = crate::agent_message_projection::compaction_message_observation(body, &self.values)
                .map_err(value_error)?;
            ara_session::native_wire_from_serde(&observed)
        } else {
            self.observe_body(body)
        }
    }

    fn observe_wire_body(&self, body: &ValueHandle) -> io::Result<WireValue> {
        self.values.check_handle(body).map_err(value_error)?;
        // Original CURRENT Gets, not export_serde, persistence preprocessing or
        // a reconstructed transcript. The existing traversal preserves all Wire
        // data fields, UTF-16 units and Number bits; graph-only values/cycles fail.
        observe_data_tree(self.values.heap(), body, self.values.runtime(), &[])
    }

    fn observe_field(&self, body: &ValueHandle, path: &[&str]) -> io::Result<Option<WireValue>> {
        self.values.check_handle(body).map_err(value_error)?;
        let heap = self.values.heap();
        let runtime = self.values.runtime();
        let mut current = body.clone();
        if path == ["content", "length"] {
            current = heap.get(body, &"content".into(), runtime).map_err(value_error)?;
            if !matches!(heap.snapshot(&current).map_err(value_error)?, ValueSnapshot::Array) {
                return Err(invalid_observation("Native recovery content is not an Array"));
            }
            current = heap.get(&current, &"length".into(), runtime).map_err(value_error)?;
            return observe_data_tree(heap, &current, runtime, path).map(Some);
        }
        for name in path {
            if matches!(heap.snapshot(&current).map_err(value_error)?, ValueSnapshot::Null | ValueSnapshot::Undefined) {
                return Ok(None);
            }
            current = heap.get(&current, &PropertyKey::from(*name), runtime).map_err(value_error)?;
        }
        if matches!(heap.snapshot(&current).map_err(value_error)?, ValueSnapshot::Undefined) {
            return Ok(None);
        }
        observe_data_tree(heap, &current, runtime, path).map(Some)
    }

    fn observe_tool_calls(&self, body: &ValueHandle) -> io::Result<Vec<(WireString, WireString)>> {
        self.values.check_handle(body).map_err(value_error)?;
        let heap = self.values.heap();
        let runtime = self.values.runtime();
        let content = heap.get(body, &"content".into(), runtime).map_err(value_error)?;
        if !matches!(heap.snapshot(&content).map_err(value_error)?, ValueSnapshot::Array) {
            return Ok(Vec::new());
        }
        let length = heap.array_length(&content, runtime).map_err(value_error)?;
        let mut calls = Vec::new();
        for index in 0..length {
            let call = heap.get(&content, &WireString::from(index.to_string()).into(), runtime).map_err(value_error)?;
            if !matches!(
                heap.snapshot(&call).map_err(value_error)?,
                ValueSnapshot::Object
                    | ValueSnapshot::Array
                    | ValueSnapshot::Function(_)
                    | ValueSnapshot::Opaque { .. }
            ) {
                continue;
            }
            let kind = heap.get(&call, &"type".into(), runtime).map_err(value_error)?;
            if !matches!(heap.snapshot(&kind).map_err(value_error)?, ValueSnapshot::String(ref kind) if kind.equals_ascii("toolCall"))
            {
                continue;
            }
            let id = heap.get(&call, &"id".into(), runtime).map_err(value_error)?;
            let name = heap.get(&call, &"name".into(), runtime).map_err(value_error)?;
            match (heap.snapshot(&id).map_err(value_error)?, heap.snapshot(&name).map_err(value_error)?) {
                (ValueSnapshot::String(id), ValueSnapshot::String(name)) => calls.push((id, name)),
                _ => {
                    return Err(invalid_observation("native tool call requires exact UTF16 id and name"));
                }
            }
        }
        Ok(calls)
    }
}

fn invalid_observation(reason: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason)
}

/// A derived observation of the requested data subtree. Each selected member
/// uses the current runtime Get. UTF16 and Number bits remain native Wire data;
/// graph-only values and cycles have an explicit error. No toJSON/replacer,
/// canonical-body replacement, callback persistence or unrelated-body walk.
fn observe_data_tree(
    heap: &Arc<ValueHeap>,
    root: &ValueHandle,
    runtime: &dyn ValueRuntime,
    requested_path: &[&str],
) -> io::Result<WireValue> {
    enum Work {
        Visit(ValueHandle),
        Array { value: ValueHandle, length: u32, index: u32, start: usize },
        Object { value: ValueHandle, keys: Vec<PropertyKey>, index: usize, selected: Vec<WireString>, start: usize },
    }
    let mut work = vec![Work::Visit(root.clone())];
    let mut output = Vec::new();
    let mut ancestors = HashSet::<NodeId>::new();
    while let Some(next) = work.pop() {
        match next {
            Work::Visit(value) => match heap.snapshot(&value).map_err(value_error)? {
                ValueSnapshot::Null => output.push(WireValue::Null),
                ValueSnapshot::Boolean(value) => output.push(WireValue::Bool(value)),
                ValueSnapshot::Number(value) => output.push(WireValue::Number(value)),
                ValueSnapshot::String(value) => output.push(WireValue::String(value)),
                ValueSnapshot::Array => {
                    if !ancestors.insert(value.node_id()) {
                        return Err(invalid_observation("native field observation has a cycle"));
                    }
                    let length = heap.array_length(&value, runtime).map_err(value_error)?;
                    work.push(Work::Array { value, length, index: 0, start: output.len() });
                }
                ValueSnapshot::Object => {
                    if !ancestors.insert(value.node_id()) {
                        return Err(invalid_observation("native field observation has a cycle"));
                    }
                    let keys = heap.own_keys(&value, runtime).map_err(value_error)?;
                    work.push(Work::Object { value, keys, index: 0, selected: Vec::new(), start: output.len() });
                }
                unsupported => {
                    // Only the requested schema path and variant name are
                    // diagnostic. Never format the original native carrier.
                    let kind = match unsupported {
                        ValueSnapshot::Undefined => "Undefined",
                        ValueSnapshot::BigInt(_) => "BigInt",
                        ValueSnapshot::Function(_) => "Function",
                        ValueSnapshot::Symbol { .. } => "Symbol",
                        ValueSnapshot::Opaque { .. } => "Opaque",
                        _ => unreachable!(),
                    };
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "native field observation at {} is not a Wire data value ({kind})",
                            requested_path.join(".")
                        ),
                    ));
                }
            },
            Work::Array { value, length, index, start } => {
                if index == length {
                    ancestors.remove(&value.node_id());
                    let children = output.split_off(start);
                    output.push(WireValue::Array(children));
                } else {
                    let child =
                        heap.get(&value, &WireString::from(index.to_string()).into(), runtime).map_err(value_error)?;
                    work.push(Work::Array { value, length, index: index + 1, start });
                    work.push(Work::Visit(child));
                }
            }
            Work::Object { value, keys, index, mut selected, start } => {
                if index == keys.len() {
                    ancestors.remove(&value.node_id());
                    let children = output.split_off(start);
                    output.push(WireValue::Object(selected.into_iter().zip(children).collect()));
                } else {
                    let key = &keys[index];
                    if heap
                        .own_descriptor(&value, key, runtime)
                        .map_err(value_error)?
                        .is_some_and(|descriptor| descriptor.enumerable())
                    {
                        let PropertyKey::String(name) = key else {
                            return Err(invalid_observation("native field observation has an enumerable Symbol"));
                        };
                        selected.push(name.clone());
                        let child = heap.get(&value, key, runtime).map_err(value_error)?;
                        work.push(Work::Object { value, keys, index: index + 1, selected, start });
                        work.push(Work::Visit(child));
                    } else {
                        work.push(Work::Object { value, keys, index: index + 1, selected, start });
                    }
                }
            }
        }
    }
    if output.len() != 1 {
        return Err(invalid_observation("native field observation produced an invalid tree"));
    }
    output.pop().ok_or_else(|| invalid_observation("native field observation has no result"))
}
