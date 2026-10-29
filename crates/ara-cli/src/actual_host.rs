//! Concrete CLI/RPC Graph consumers. Fixed OMP 596f2da (MIT; THIRD_PARTY.md).
//! Provider context and canonical messages never pass through a typed mirror.
//! Terminal fields, Broker rows and public JSON are explicit derived boundaries.
use ara_agent::{ActualProviderContext, AgentMessage, AgentMessageBody, RawContextError, SessionValues};
use ara_ai::StopReason;
use ara_rpc::{
    WireString, WireValue,
    value_heap::{
        CompiledFunctionForm, CompiledFunctionSpec, NodeId, PropertyDescriptor, PropertyKey, StringifyMode, ValueError,
        ValueHandle, ValueHeap, ValueKind, ValueRuntime, ValueSnapshot,
    },
};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

pub(crate) struct HostGraph<'a>(pub &'a SessionValues);
impl HostGraph<'_> {
    pub fn get(&self, root: &ValueHandle, key: &str) -> Result<ValueHandle, ValueError> {
        self.0.check_handle(root)?;
        self.0.heap().get(root, &PropertyKey::from(key), self.0.runtime())
    }
    pub fn put(&self, root: &ValueHandle, key: &str, value: ValueHandle) -> Result<(), ValueError> {
        self.0.heap().define_own_data(root, PropertyKey::from(key), value, self.0.runtime())
    }
    pub fn string(&self, value: &ValueHandle) -> Result<WireString, ValueError> {
        match self.0.heap().snapshot(value)? {
            ValueSnapshot::String(value) => Ok(value),
            _ => Err(ValueError::TypeError("Host string field must be a string".into())),
        }
    }
    pub fn is(&self, root: &ValueHandle, key: &str, expected: &str) -> Result<bool, ValueError> {
        Ok(
            matches!(self.0.heap().snapshot(&self.get(root, key)?)?, ValueSnapshot::String(value) if value.equals_ascii(expected)),
        )
    }
    pub fn nullish(&self, value: &ValueHandle) -> Result<bool, ValueError> {
        Ok(matches!(self.0.heap().snapshot(value)?, ValueSnapshot::Undefined | ValueSnapshot::Null))
    }
    pub fn text(&self, value: impl Into<WireString>) -> Result<ValueHandle, ValueError> {
        self.0.heap().string(value.into())
    }
    pub fn members(&self, value: &ValueHandle) -> Result<Vec<ValueHandle>, ValueError> {
        let mut iterator = self.0.heap().get_iterator(value, self.0.runtime())?;
        let mut out = Vec::new();
        loop {
            match self.0.heap().iterator_next(&mut iterator, self.0.runtime()) {
                Ok(Some(value)) => out.push(value),
                Ok(None) => return Ok(out),
                Err(primary) => {
                    let _ = self.0.heap().iterator_close(&mut iterator, self.0.runtime());
                    return Err(primary);
                }
            }
        }
    }
    pub fn array(&self, members: impl IntoIterator<Item = ValueHandle>) -> Result<ValueHandle, ValueError> {
        let array = self.0.heap().array_with_length(0)?;
        for (index, member) in members.into_iter().enumerate() {
            self.0.heap().define_own_data(
                &array,
                WireString::from(index.to_string()).into(),
                member,
                self.0.runtime(),
            )?;
        }
        Ok(array)
    }
    pub fn block(&self, text: impl Into<WireString>) -> Result<ValueHandle, ValueError> {
        let block = self.0.heap().object()?;
        self.put(&block, "type", self.text("text")?)?;
        self.put(&block, "text", self.text(text)?)?;
        Ok(block)
    }
    pub fn spread(&self, root: &ValueHandle) -> Result<ValueHandle, ValueError> {
        self.0.heap().spread(root, self.0.runtime())
    }
    pub fn message(&self, role: &str, content: ValueHandle, source: &ValueHandle) -> Result<ValueHandle, ValueError> {
        let message = self.0.heap().object()?;
        self.put(&message, "role", self.text(WireString::from(role))?)?;
        self.put(&message, "content", content)?;
        self.put(&message, "attribution", self.get(source, "attribution")?)?;
        self.put(&message, "timestamp", self.get(source, "timestamp")?)?;
        Ok(message)
    }
}
pub(crate) fn join(parts: &[WireString], separator: &str) -> WireString {
    let mut out = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        if index != 0 {
            out.extend(separator.encode_utf16());
        }
        out.extend_from_slice(part.units());
    }
    WireString::from_units(out)
}
pub(crate) fn scalar(value: &WireString) -> Result<String, ValueError> {
    value.to_utf8().map_err(|_| ValueError::LegacyBridge("Host OS/display string is not Unicode scalar text"))
}
pub(crate) fn trimmed_empty(value: &WireString) -> bool {
    value.units().iter().all(|unit| char::from_u32(u32::from(*unit)).is_some_and(ara_prompt::js::is_space))
}

#[derive(Clone, Debug)]
pub enum AssistantDisplayBlock {
    Text(String),
    Thinking(String),
}
#[derive(Clone, Debug)]
pub struct AssistantDisplay {
    pub content: Vec<AssistantDisplayBlock>,
    pub stop_reason: StopReason,
    pub error_message: Option<String>,
}
impl AssistantDisplay {
    /// This DTO contains only the fields actually read by main's progress,
    /// print-thoughts and exit-cause consumers. It is never provider input.
    pub fn read(message: &AgentMessage, values: Option<&SessionValues>) -> Result<Option<Self>, RawContextError> {
        let AgentMessageBody::Graph(root) = message.body_snapshot() else {
            let Some(ara_ai::Message::Assistant(message)) = message.checked_model_snapshot()? else {
                return Ok(None);
            };
            return Ok(Some(Self {
                content: message
                    .content
                    .into_iter()
                    .filter_map(|block| match block {
                        ara_ai::AssistantBlock::Text(text) => Some(AssistantDisplayBlock::Text(text.text)),
                        ara_ai::AssistantBlock::Thinking(thinking) => {
                            Some(AssistantDisplayBlock::Thinking(thinking.thinking))
                        }
                        _ => None,
                    })
                    .collect(),
                stop_reason: message.stop_reason,
                error_message: message.error_message,
            }));
        };
        let values = values.ok_or_else(|| RawContextError("actual Host values are not bound".into()))?;
        read_display(&HostGraph(values), &root).map_err(|error| RawContextError(error.to_string()))
    }
}
fn read_display(graph: &HostGraph<'_>, root: &ValueHandle) -> Result<Option<AssistantDisplay>, ValueError> {
    if !graph.is(root, "role", "assistant")? {
        return Ok(None);
    }
    let mut content = Vec::new();
    for block in graph.members(&graph.get(root, "content")?)? {
        if graph.is(&block, "type", "text")? {
            content.push(AssistantDisplayBlock::Text(scalar(&graph.string(&graph.get(&block, "text")?)?)?));
        } else if graph.is(&block, "type", "thinking")? {
            content.push(AssistantDisplayBlock::Thinking(scalar(&graph.string(&graph.get(&block, "thinking")?)?)?));
        }
    }
    let stop_reason = match scalar(&graph.string(&graph.get(root, "stopReason")?)?)?.as_str() {
        "stop" => StopReason::Stop,
        "length" => StopReason::Length,
        "toolUse" => StopReason::ToolUse,
        "error" => StopReason::Error,
        "aborted" => StopReason::Aborted,
        _ => return Err(ValueError::TypeError("Unknown assistant display stop reason".into())),
    };
    let error = graph.get(root, "errorMessage")?;
    let error_message = if graph.nullish(&error)? { None } else { Some(scalar(&graph.string(&error)?)?) };
    Ok(Some(AssistantDisplay { content, stop_reason, error_message }))
}

/// Reads only the existing AuthStorage.record_assistant_usage row fields.
/// Missing/null/nonfinite/nonintegral buckets retain the existing no-report
/// contract; unknown usage never becomes a zero bucket.
pub fn observed_usage(
    root: &ValueHandle,
    values: &SessionValues,
) -> Result<Option<crate::credential_store::ClientUsageEntry>, ValueError> {
    let graph = HostGraph(values);
    if !graph.is(root, "role", "assistant")? {
        return Ok(None);
    }
    let usage = graph.get(root, "usage")?;
    if graph.nullish(&usage)? {
        return Ok(None);
    }
    fn count(graph: &HostGraph<'_>, root: &ValueHandle, key: &str) -> Result<Option<i64>, ValueError> {
        Ok(match graph.0.heap().snapshot(&graph.get(root, key)?)? {
            ValueSnapshot::Number(value)
                if value.is_finite() && value >= 0.0 && value < 9_223_372_036_854_775_808.0 && value.fract() == 0.0 =>
            {
                Some(value as i64)
            }
            _ => None,
        })
    }
    let (Some(input_tokens), Some(output_tokens), Some(cache_read_tokens), Some(cache_write_tokens)) = (
        count(&graph, &usage, "input")?,
        count(&graph, &usage, "output")?,
        count(&graph, &usage, "cacheRead")?,
        count(&graph, &usage, "cacheWrite")?,
    ) else {
        return Ok(None);
    };
    let cost = graph.get(&usage, "cost")?;
    if graph.nullish(&cost)? {
        return Ok(None);
    }
    let ValueSnapshot::Number(cost_usd) = values.heap().snapshot(&graph.get(&cost, "total")?)? else {
        return Ok(None);
    };
    if !cost_usd.is_finite() {
        return Ok(None);
    }
    let ValueSnapshot::Number(at) = values.heap().snapshot(&graph.get(root, "timestamp")?)? else {
        return Err(ValueError::TypeError("Broker usage timestamp must be a number".into()));
    };
    if !at.is_finite()
        || at.fract() != 0.0
        || !(-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&at)
    {
        return Err(ValueError::LegacyBridge("Broker timestamp is outside i64 milliseconds"));
    }
    Ok(Some(crate::credential_store::ClientUsageEntry {
        at: at as i64,
        provider: scalar(&graph.string(&graph.get(root, "provider")?)?)?,
        model: scalar(&graph.string(&graph.get(root, "model")?)?)?,
        requests: 1,
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_write_tokens,
        cost_usd,
    }))
}

#[derive(Default)]
struct ReminderState {
    root: Option<ValueHandle>,
    current: Option<String>,
    injections: Vec<(ValueHandle, ValueHandle)>,
    controls: Option<ValueHandle>,
    seen: HashSet<NodeId>,
}
#[derive(Default)]
pub struct GraphDateCwdReminder {
    state: Mutex<ReminderState>,
}
impl GraphDateCwdReminder {
    pub fn transform(
        &self,
        mut context: ActualProviderContext,
        values: &SessionValues,
        date: &str,
        cwd: &str,
    ) -> Result<ActualProviderContext, ara_agent::ActualProviderError> {
        context.check_lineage(values)?;
        let graph = HostGraph(values);
        let root = context
            .original_context
            .as_ref()
            .ok_or(ara_agent::ActualProviderError::Unsupported("DateCwdReminder original Context"))?
            .clone();
        // Source transform reads systemPrompt twice and messages once for the
        // guard. Strict === 0 does not ToLength/coerce a programmable length.
        if !values.heap().truthy(&graph.get(&root, "systemPrompt")?)?
            || is_zero(values.heap(), &graph.get(&graph.get(&root, "systemPrompt")?, "length")?)?
            || is_zero(values.heap(), &graph.get(&graph.get(&root, "messages")?, "length")?)?
        {
            return Ok(context);
        }
        let reminder = ara_context::render_date_cwd_reminder(date, cwd);
        let messages = self.inject(&graph, &graph.get(&root, "messages")?, &reminder)?;
        if strict_equal(values.heap(), &messages, &graph.get(&root, "messages")?)? {
            return Ok(context);
        }
        let replacement = graph.spread(&root)?;
        graph.put(&replacement, "messages", messages)?;
        context.original_context = Some(replacement);
        // The old Vec has no original-container contract after a Native
        // replacement. Do not materialize a new mirror by extra iteration.
        context.messages.clear();
        Ok(context)
    }

    fn with_state<T>(&self, operation: impl FnOnce(&mut ReminderState) -> T) -> T {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        operation(&mut state)
    }

    fn inject(&self, graph: &HostGraph<'_>, messages: &ValueHandle, reminder: &str) -> Result<ValueHandle, ValueError> {
        // Stateless retained arrow: no Host, SessionValues or runtime lease.
        let find = graph.get(messages, "find")?;
        let predicate = graph.0.heap().fresh_compiled_function(&FIRST_USER, &[])?;
        let first_user = graph.0.heap().call(&find, messages, &[predicate], graph.0.runtime())?;
        if !graph.0.heap().truthy(&first_user)? {
            return Ok(messages.clone());
        }
        let prior_root = self.with_state(|state| state.root.clone());
        let same_root = match prior_root {
            Some(prior) => strict_equal(graph.0.heap(), &prior, &first_user)?,
            None => false,
        };
        if !same_root {
            let controls = graph.0.heap().array(Vec::new())?;
            self.with_state(|state| {
                *state = ReminderState {
                    root: Some(first_user.clone()),
                    current: Some(reminder.to_string()),
                    controls: Some(controls),
                    ..ReminderState::default()
                }
            });
            if !starts_with_reminder(graph, &first_user, reminder)? {
                let injected = inject_reminder(graph, &first_user, reminder)?;
                self.set_injection(first_user, injected);
            }
        } else if self.with_state(|state| state.current.as_deref() != Some(reminder)) {
            let mut new_user = None;
            let length = graph.get(messages, "length")?;
            let mut index = graph.0.heap().to_number(&length, graph.0.runtime())? - 1.0;
            while index >= 0.0 {
                let key = graph.0.heap().to_string(&graph.0.heap().number(index)?, graph.0.runtime())?;
                let candidate = graph.0.heap().get(messages, &key.into(), graph.0.runtime())?;
                if graph.is(&candidate, "role", "user")?
                    && !self.with_state(|state| state.seen.contains(&candidate.node_id()))
                {
                    new_user = Some(candidate);
                    break;
                }
                index -= 1.0;
            }
            if let Some(user) = new_user {
                let injected = inject_reminder(graph, &user, reminder)?;
                self.set_injection(user, injected);
            } else {
                let at = graph.get(messages, "at")?;
                let anchor = graph.0.heap().call(&at, messages, &[graph.0.heap().number(-1.0)?], graph.0.runtime())?;
                // Source gets controls.push before evaluating its object arg.
                let controls = self
                    .with_state(|state| state.controls.clone())
                    .ok_or(ValueError::WrongRuntimeReply("Reminder controls owner"))?;
                let push = graph.get(&controls, "push")?;
                let developer = graph.0.heap().object()?;
                graph.put(&developer, "role", graph.text("developer")?)?;
                graph.put(&developer, "content", graph.text(WireString::from(reminder))?)?;
                graph.put(&developer, "synthetic", graph.0.heap().boolean(true)?)?;
                graph.put(&developer, "timestamp", graph.0.heap().number(ara_ai::now_ms() as f64)?)?;
                let control = graph.0.heap().object()?;
                graph.put(&control, "anchor", anchor)?;
                graph.put(&control, "message", developer)?;
                graph.0.heap().call(&push, &controls, &[control], graph.0.runtime())?;
            }
            self.with_state(|state| state.current = Some(reminder.to_string()));
        }

        // Maps/WeakSet below are Host identity metadata until neutral Native
        // Map/WeakSet primitives are joined. Their prototype/weak semantics
        // remain explicit open work; array/Get/callback operations are actual.
        let mut controls_by_anchor: Vec<(ValueHandle, ValueHandle)> = Vec::new();
        let controls = self
            .with_state(|state| state.controls.clone())
            .ok_or(ValueError::WrongRuntimeReply("Reminder controls owner"))?;
        for_of(graph, &controls, |control| {
            let anchor = graph.get(control, "anchor")?;
            let collection = controls_by_anchor
                .iter()
                .find(|(source, _)| source.same_identity(&anchor))
                .map(|(_, collection)| collection.clone());
            if let Some(collection) = collection {
                let push = graph.get(&collection, "push")?;
                let message = graph.get(control, "message")?;
                graph.0.heap().call(&push, &collection, &[message], graph.0.runtime())?;
            } else {
                let anchor = graph.get(control, "anchor")?;
                let message = graph.get(control, "message")?;
                controls_by_anchor.push((anchor, graph.0.heap().array(vec![message])?));
            }
            Ok(())
        })?;
        let mut changed = false;
        let out = graph.0.heap().array(Vec::new())?;
        for_of(graph, messages, |message| {
            let injected = self.with_state(|state| {
                state
                    .injections
                    .iter()
                    .find(|(source, _)| source.same_identity(message))
                    .map(|(_, injected)| injected.clone())
            });
            let push = graph.get(&out, "push")?;
            graph.0.heap().call(
                &push,
                &out,
                &[injected.clone().unwrap_or_else(|| message.clone())],
                graph.0.runtime(),
            )?;
            if injected.is_some() {
                changed = true;
            }
            if let Some((_, controls)) = controls_by_anchor.iter().find(|(anchor, _)| anchor.same_identity(message)) {
                let push = graph.get(&out, "push")?;
                let mut arguments = Vec::new();
                for_of(graph, controls, |message| {
                    arguments.push(message.clone());
                    Ok(())
                })?;
                graph.0.heap().call(&push, &out, &arguments, graph.0.runtime())?;
                changed = true;
            }
            if !matches!(graph.0.heap().kind(message)?, ValueKind::Object | ValueKind::Array | ValueKind::Function) {
                return Err(ValueError::TypeError("Invalid value used in weak set".into()));
            }
            self.with_state(|state| {
                state.seen.insert(message.node_id());
            });
            Ok(())
        })?;
        Ok(if changed { out } else { messages.clone() })
    }

    fn set_injection(&self, source: ValueHandle, injected: ValueHandle) {
        self.with_state(|state| {
            if let Some((_, prior)) = state.injections.iter_mut().find(|(message, _)| message.same_identity(&source)) {
                *prior = injected;
            } else {
                state.injections.push((source, injected));
            }
        });
    }
}
static FIRST_USER: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "date-cwd-reminder.ts:61",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 0,
    body: first_user_predicate,
};
fn first_user_predicate(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    arguments: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    let message = match arguments.first() {
        Some(message) => message.clone(),
        None => heap.undefined()?,
    };
    let role = heap.get(&message, &"role".into(), runtime)?;
    heap.boolean(matches!(heap.snapshot(&role)?, ValueSnapshot::String(role) if role.equals_ascii("user")))
}
fn is_zero(heap: &Arc<ValueHeap>, value: &ValueHandle) -> Result<bool, ValueError> {
    Ok(matches!(heap.snapshot(value)?, ValueSnapshot::Number(number) if number == 0.0))
}
fn strict_equal(heap: &Arc<ValueHeap>, left: &ValueHandle, right: &ValueHandle) -> Result<bool, ValueError> {
    match (heap.snapshot(left)?, heap.snapshot(right)?) {
        (ValueSnapshot::Number(left), ValueSnapshot::Number(right)) => Ok(left == right),
        _ => heap.object_is(left, right),
    }
}
fn for_of(
    graph: &HostGraph<'_>,
    source: &ValueHandle,
    mut body: impl FnMut(&ValueHandle) -> Result<(), ValueError>,
) -> Result<(), ValueError> {
    let mut iterator = graph.0.heap().get_iterator(source, graph.0.runtime())?;
    while let Some(value) = graph.0.heap().iterator_next(&mut iterator, graph.0.runtime())? {
        if let Err(primary) = body(&value) {
            let close = graph.0.heap().iterator_close(&mut iterator, graph.0.runtime());
            return Err(match close {
                Ok(()) => primary,
                Err(close) if primary.is_native_exception() && close.is_native_exception() => primary,
                Err(close) => ValueError::IteratorClose { original: Box::new(primary), close: Box::new(close) },
            });
        }
    }
    Ok(())
}
fn starts_with_reminder(graph: &HostGraph<'_>, user: &ValueHandle, reminder: &str) -> Result<bool, ValueError> {
    let content = graph.get(user, "content")?;
    if graph.0.heap().kind(&content)? == ValueKind::String {
        let content = graph.get(user, "content")?;
        let method = graph.get(&content, "startsWith")?;
        let matched =
            graph.0.heap().call(&method, &content, &[graph.text(WireString::from(reminder))?], graph.0.runtime())?;
        return graph.0.heap().truthy(&matched);
    }
    let first = graph.get(&graph.get(user, "content")?, "0")?;
    if graph.nullish(&first)? || !graph.is(&first, "type", "text")? {
        return Ok(false);
    }
    let first = graph.get(&graph.get(user, "content")?, "0")?;
    strict_equal(graph.0.heap(), &graph.get(&first, "text")?, &graph.text(WireString::from(reminder))?)
}
fn inject_reminder(graph: &HostGraph<'_>, user: &ValueHandle, reminder: &str) -> Result<ValueHandle, ValueError> {
    let source_content = graph.get(user, "content")?;
    let content = match graph.0.heap().kind(&source_content)? {
        ValueKind::String => {
            let content = graph.get(user, "content")?;
            let content = graph.0.heap().to_string(&content, graph.0.runtime())?;
            graph.text(join(&[WireString::from(reminder), content], "\n\n"))?
        }
        _ => {
            let mut blocks = vec![graph.block(WireString::from(reminder))?];
            let source_content = graph.get(user, "content")?;
            for_of(graph, &source_content, |block| {
                blocks.push(block.clone());
                Ok(())
            })?;
            graph.0.heap().array(blocks)?
        }
    };
    let message = graph.spread(user)?;
    graph.put(&message, "content", content)?;
    Ok(message)
}

/// Source JSON serialization is permitted only at this public display boundary.
/// Undefined is omitted by stringify; canonical Graph is never replaced.
pub(crate) fn public_json(root: &ValueHandle, values: &SessionValues) -> Result<serde_json::Value, ValueError> {
    legacy_public_wire(&public_wire(root, values, true)?)
}

/// Same public redaction/serialization; exact wire units remain a derived view.
pub(crate) fn public_wire(root: &ValueHandle, values: &SessionValues, legacy_display: bool) -> Result<WireValue, ValueError> {
    let graph = HostGraph(values);
    let source_assistant = graph.is(root, "role", "assistant")?;
    let public = if source_assistant {
        let public = values.heap().spread_excluding(root, &["providerPayload".into()], values.runtime())?;
        let mut content = Vec::new();
        for block in graph.members(&graph.get(root, "content")?)? {
            content.push(if graph.is(&block, "type", "thinking")? {
                values.heap().spread_excluding(&block, &["thinkingSignature".into()], values.runtime())?
            } else {
                block
            });
        }
        graph.put(&public, "content", graph.array(content)?)?;
        public
    } else {
        root.clone()
    };
    let mut json = wire_data(&public, values)?;
    // Legacy refuses before final redaction, exactly where json_data did.
    if legacy_display { legacy_public_wire(&json)?; }
    // Own toJSON is source behavior and can return the original message/block.
    // Redact again at the final public boundary, using the admitted source role.
    // Business arguments/details are not assistant protocol metadata.
    if source_assistant && let WireValue::Object(fields) = &mut json {
        fields.retain(|(name, _)| !name.equals_ascii("providerPayload"));
        if let Some((_, WireValue::Array(blocks))) = fields.iter_mut().find(|(name, _)| name.equals_ascii("content")) {
            for block in blocks {
                if block.get("type").and_then(WireValue::as_string).is_some_and(|kind| kind.equals_ascii("thinking"))
                    && let WireValue::Object(fields) = block
                {
                    fields.retain(|(name, _)| !name.equals_ascii("thinkingSignature"));
                }
            }
        }
    }
    Ok(json)
}
/// Check only the freshly generated trusted loop notice at Host input admission.
/// Own data reads do not invoke the Native detector/render callbacks again.
/// A refusal retains its post-render details at the explicit public boundary;
/// neither the canonical custom Graph nor its detection becomes a typed DTO.
pub fn loop_notice_public_utf16_refusal(
    custom: &ValueHandle,
    values: &SessionValues,
) -> Result<Option<WireValue>, ValueError> {
    values.check_handle(custom)?;
    let heap = values.heap();
    let runtime = values.runtime();
    let data = |root: &ValueHandle, name: &str| -> Result<ValueHandle, ValueError> {
        match heap.own_descriptor(root, &name.into(), runtime)? {
            Some(PropertyDescriptor::Data { value, .. }) => Ok(value),
            _ => Err(ValueError::UnsupportedOperation("Fresh loop notice requires own data fields")),
        }
    };
    let unpaired = |value: &ValueHandle| -> Result<bool, ValueError> {
        Ok(matches!(heap.snapshot(value)?, ValueSnapshot::String(value) if value.to_utf8().is_err()))
    };
    let details = data(custom, "details")?;
    let mut refused = unpaired(&data(custom, "content")?)?;
    for name in ["toolName", "argumentsSummary", "resultSummary"] {
        refused |= unpaired(&data(&details, name)?)?;
    }
    if !refused {
        return Ok(None);
    }
    // Only the refusal serializes here. Ordinary notices retain the existing
    // single public-event serialization and its original toJSON/Get behavior.
    let json = heap
        .stringify(&details, StringifyMode::Plain, None, runtime)?
        .ok_or(ValueError::TypeError("Loop notice details stringify returned undefined".into()))?;
    WireValue::parse_units(json.units()).map(Some).map_err(ValueError::StrictParse)
}

pub(crate) fn json_data(root: &ValueHandle, values: &SessionValues) -> Result<serde_json::Value, ValueError> {
    legacy_public_wire(&wire_data(root, values)?)
}

pub(crate) fn legacy_public_wire(wire: &WireValue) -> Result<serde_json::Value, ValueError> {
    serde_json::from_str(&wire.stringify())
        .map_err(|_| ValueError::LegacyBridge("Public JSON cannot enter the scalar display bridge"))
}

pub(crate) fn wire_data(root: &ValueHandle, values: &SessionValues) -> Result<WireValue, ValueError> {
    let json = values
        .heap()
        .stringify(root, StringifyMode::Plain, None, values.runtime())?
        .ok_or(ValueError::TypeError("Public event root is undefined".into()))?;
    WireValue::parse_units(json.units()).map_err(ValueError::StrictParse)
}
