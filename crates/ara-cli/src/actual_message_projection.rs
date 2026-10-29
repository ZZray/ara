//! Original-domain conversion consumers, fixed coding-agent/session/messages.ts
//! and agent/compaction/messages.ts at 596f2da (MIT; THIRD_PARTY.md).
use super::*;
use crate::actual_host::{HostGraph, join, trimmed_empty};
use ara_agent::{ActualProviderError, SessionValues};
use ara_rpc::{
    WireString, WireValue,
    value_heap::{ValueHandle, ValueSnapshot},
};

pub(super) struct ActualMemo {
    generation: u64,
    /// Generation last observed while this original root had a pass-through alias.
    alias_version_generation: Option<u64>,
    root: ValueHandle,
    interrupted_next: bool,
    route: Route,
    fragment: Vec<ValueHandle>,
}

impl AgentMessageProjector {
    /// Owned comparison observations of local-reduction fields. Graph roots
    /// retain their canonical source and original Host encoder throughout.
    pub fn reduction_receipts(&self, messages: &[AgentMessage]) -> Result<Vec<Value>, RawContextError> {
        let encoder = self.native_encoder.lock().unwrap().clone();
        messages
            .iter()
            .map(|message| match message.body_snapshot() {
                ara_agent::AgentMessageBody::Graph(root) => {
                    if self.current_role(message)? {
                        return Ok(Self::wire_receipt(self.current_wire_body(message)?.as_ref()));
                    }
                    let encoder = encoder.as_ref().ok_or_else(|| error("local reducer has no native Host encoder"))?;
                    NativeMessageSource::live(root, encoder.clone()).checked_reduction_body().map_err(error)
                }
                _ => message.json(),
            })
            .collect()
    }

    pub fn actual_values(&self) -> Option<Arc<SessionValues>> {
        self.native_encoder.lock().unwrap().as_ref().map(|encoder| encoder.values().clone())
    }
    /// This runs once at an existing Host input/reopen boundary. It does not
    /// import a provider stream, transform result or request-time Graph body.
    pub fn new_legacy_message(&self, body: Value, source: Option<Arc<Value>>) -> Result<AgentMessage, RawContextError> {
        if let Some(values) = self.actual_values() {
            let root = values.heap().import_serde(&body).map_err(error)?;
            AgentMessage::from_graph(root, source).map_err(error)
        } else {
            AgentMessage::from_raw(body, source)
        }
    }
    pub fn display_assistant(
        &self,
        message: &AgentMessage,
    ) -> Result<Option<crate::actual_host::AssistantDisplay>, RawContextError> {
        let values = self.actual_values();
        crate::actual_host::AssistantDisplay::read(message, values.as_deref())
    }
    pub fn observed_actual_usage(
        &self,
        message: &AgentMessage,
    ) -> Result<Option<crate::credential_store::ClientUsageEntry>, RawContextError> {
        let ara_agent::AgentMessageBody::Graph(root) = message.body_snapshot() else {
            return Ok(None);
        };
        let values = self.actual_values().ok_or_else(|| error("actual Host values are not bound"))?;
        crate::actual_host::observed_usage(&root, &values).map_err(error)
    }
    pub fn new_custom_input(&self, body: Value) -> Result<AgentMessage, RawContextError> {
        Entry::project_agent_message_body(&body).map_err(error)?;
        let source = Some(Arc::new(json!({"origin":body.get("customType"), "attribution":body.get("attribution")})));
        self.new_legacy_message(body, source)
    }
    /// Existing terminal activity-summary consumes only these OS/display
    /// string fields. The original argument object remains the execute input.
    pub fn activity_field(&self, args: &ValueHandle, name: &str) -> Result<Option<String>, RawContextError> {
        let values = self.actual_values().ok_or_else(|| error("actual Host values are not bound"))?;
        let graph = HostGraph(&values);
        let value = graph.get(args, name).map_err(error)?;
        match values.heap().snapshot(&value).map_err(error)? {
            ValueSnapshot::String(value) => crate::actual_host::scalar(&value).map(Some).map_err(error),
            _ => Ok(None),
        }
    }
    /// main's JSON printer and RPC's public snapshot are explicit derived
    /// boundaries. Serialize original native values here, never in converter,
    /// preparation, execution or journal ownership.
    pub fn public_event(
        &self,
        event: &ara_agent::AgentEvent,
        include_snapshot: bool,
    ) -> Result<Value, RawContextError> {
        if self.actual_values().is_none() {
            return if include_snapshot { event.full() } else { event.printable() };
        }
        crate::actual_host::legacy_public_wire(&self.public_event_carrier(event, include_snapshot, true)?).map_err(error)
    }
    /// One public event projector. RPC keeps this wire carrier; legacy display
    /// callers explicitly retain the checked scalar bridge above.
    pub fn public_event_wire(
        &self,
        event: &ara_agent::AgentEvent,
        include_snapshot: bool,
    ) -> Result<WireValue, RawContextError> {
        self.public_event_carrier(event, include_snapshot, false)
    }
    fn public_event_carrier(
        &self,
        event: &ara_agent::AgentEvent,
        include_snapshot: bool,
        legacy_display: bool,
    ) -> Result<WireValue, RawContextError> {
        use crate::actual_host::{wire_data, public_wire};
        use ara_agent::AgentEvent;
        let legacy = |value: Value| WireValue::parse(&value.to_string()).map_err(error);
        let Some(values) = self.actual_values() else {
            return legacy(if include_snapshot { event.full()? } else { event.printable()? });
        };
        // Keep strict legacy failures at their previous observation point;
        // later source Gets/toJSON must not run after an earlier refusal.
        if legacy_display {
            match event {
                AgentEvent::ActualToolExecutionStart { tool_call_id, tool_name, .. }
                | AgentEvent::ActualToolExecutionUpdate { tool_call_id, tool_name, .. }
                | AgentEvent::ActualToolExecutionEnd { tool_call_id, tool_name, .. } => {
                    crate::actual_host::scalar(tool_call_id).map_err(error)?;
                    crate::actual_host::scalar(tool_name).map_err(error)?;
                }
                _ => {}
            }
        }
        let checked = |wire: WireValue| -> Result<WireValue, RawContextError> {
            if legacy_display { crate::actual_host::legacy_public_wire(&wire).map_err(error)?; }
            Ok(wire)
        };
        let data = |root: &ValueHandle| checked(wire_data(root, &values).map_err(error)?);
        let public_body = |root: &ValueHandle| public_wire(root, &values, legacy_display).map_err(error);
        let body = |message: &AgentMessage| -> Result<WireValue, RawContextError> {
            match message.body_snapshot() {
                ara_agent::AgentMessageBody::Graph(root) => public_body(&root),
                _ => legacy(AgentEvent::MessageEnd { message: message.clone() }.printable()?["message"].clone()),
            }
        };
        let name = WireValue::String(event.type_name().into());
        let text = |value: String| WireValue::String(value.into());
        Ok(match event {
            AgentEvent::MessageStart { message } | AgentEvent::MessageEnd { message } => {
                WireValue::object(vec![("type", name), ("message", body(message)?)])
            }
            AgentEvent::ActualMessageEnd { message, finalization_error } => {
                WireValue::object(vec![("type", name), ("message", body(message)?), ("finalizationError", text(finalization_error.to_string()))])
            }
            AgentEvent::AgentEnd { messages } => {
                WireValue::object(vec![("type", name), ("messages", WireValue::Array(messages.iter().map(body).collect::<Result<Vec<_>,_>>()?))])
            }
            AgentEvent::ActualAgentEnd { messages, failure } => {
                WireValue::object(vec![("type", name), ("messages", WireValue::Array(messages.iter().map(body).collect::<Result<Vec<_>,_>>()?)), ("failure", text(failure.to_string()))])
            }
            AgentEvent::ActualTurnEnd { message, tool_results, preparation_error } => {
                let mut public = WireValue::object(vec![("type", name), ("message", body(message)?),
                    ("toolResults", WireValue::Array(tool_results.iter().map(body).collect::<Result<Vec<_>,_>>()?))]);
                if let Some(failure) = preparation_error { public.insert("preparationError", text(failure.to_string())); }
                public
            }
            AgentEvent::ActualMessageUpdate { message, event: update } => {
                use ara_ai::AssistantMessageEvent as E;
                let original = update.printable_graph(values.runtime()).map_err(error)?;
                let mut update_json = data(&original)?;
                if include_snapshot {
                    let field = match update { E::Done { .. } => "message", E::Error { .. } => "error", _ => "partial" };
                    update_json.insert(field, public_body(update.partial())?);
                    WireValue::object(vec![("type", name), ("message", body(message)?), ("assistantMessageEvent", update_json)])
                } else { WireValue::object(vec![("type", name), ("assistantMessageEvent", update_json)]) }
            }
            AgentEvent::ActualToolExecutionStart { tool_call_id, tool_name, args, intent } => {
                let mut public = WireValue::object(vec![("type", name), ("toolCallId", WireValue::String(tool_call_id.clone())),
                    ("toolName", WireValue::String(tool_name.clone())), ("args", data(args)?)]);
                if !matches!(values.heap().snapshot(intent).map_err(error)?, ValueSnapshot::Undefined) {
                    public.insert("intent", data(intent)?);
                }
                public
            }
            AgentEvent::ActualToolExecutionUpdate { tool_call_id, tool_name, partial, .. } => {
                WireValue::object(vec![("type", name), ("toolCallId", WireValue::String(tool_call_id.clone())),
                    ("toolName", WireValue::String(tool_name.clone())), ("partialResult", data(partial)?)])
            }
            AgentEvent::ActualToolExecutionEnd { tool_call_id, tool_name, result, is_error } => {
                WireValue::object(vec![("type", name), ("toolCallId", WireValue::String(tool_call_id.clone())),
                    ("toolName", WireValue::String(tool_name.clone())), ("result", data(result)?), ("isError", WireValue::Bool(*is_error))])
            }
            _ => legacy(if include_snapshot { event.full()? } else { event.printable()? })?,
        })
    }
    pub fn convert_current_actual(
        &self,
        messages: &[AgentMessage],
        model: &Model,
        values: &SessionValues,
    ) -> Result<Vec<ValueHandle>, ActualProviderError> {
        let bound = self.actual_values().ok_or(ActualProviderError::Unsupported("actual Host journal binding"))?;
        if !bound.same_lineage(values) {
            return Err(ara_rpc::value_heap::ValueError::ForeignHeap.into());
        }
        let (available, images) = self.settings_for(model);
        let route = self.route(model, available, images)
            .map_err(|error| ActualProviderError::Callback(error.to_string()))?;
        let graph = HostGraph(values);
        let roots = messages
            .iter()
            .map(|message| {
                let ara_agent::AgentMessageBody::Graph(root) = message.body_snapshot() else {
                    return Err(ActualProviderError::Unsupported(
                        "Host input must be admitted at creation/reopen boundary",
                    ));
                };
                values.check_handle(&root)?;
                Ok(root)
            })
            .collect::<Result<Vec<_>, ActualProviderError>>()?;
        self.actual_cache.lock().unwrap().retain(|identity, _| identity.upgrade().is_some());
        let mut output = Vec::new();
        for (index, (message, root)) in messages.iter().zip(&roots).enumerate() {
            let identity = message.weak_identity();
            let generation = message.derivation_generation();
            let interrupted_next = if let Some(next) = roots.get(index + 1) {
                graph.is(next, "role", "custom")? && graph.is(next, "customType", "interrupted-thinking")?
            } else {
                false
            };
            let (cached, mut alias_version_generation) = {
                let cache = self.actual_cache.lock().unwrap();
                let previous = cache.get(&identity);
                let same_root = previous.is_some_and(|memo| memo.root.same_identity(root));
                let cached = previous
                    .filter(|memo| {
                        memo.generation == generation
                            && same_root
                            && memo.interrupted_next == interrupted_next
                            && memo.route == route
                    })
                    .map(|memo| memo.fragment.clone());
                let alias_generation = previous.filter(|_| same_root).and_then(|memo| memo.alias_version_generation);
                (cached, alias_generation)
            };
            if alias_version_generation.is_some_and(|previous| previous != generation) {
                // Same canonical root can alias an earlier append log even if this
                // conversion now produces a derived fragment. Publish before spread,
                // outside all projector/record locks; unchanged converts do no write/Get.
                ara_agent::append_only_context::actual::publish_owner_estimate_version_actual(
                    root, generation, values,
                )?;
                alias_version_generation = Some(generation);
            }
            // A summary grant is tied to its complete CURRENT source. Recheck it
            // even when a nested mutation did not replace the outer record.
            let summary = graph.is(root, "role", "compactionSummary")?;
            let checked = if summary {
                self.native_projection_for_current(message, &route)
                    .map_err(|error| ActualProviderError::Callback(error.to_string()))?
            } else {
                None
            };
            let cached = if summary { None } else { cached };
            let fragment = match cached {
                Some(fragment) => fragment,
                None => {
                    let fragment = if let Some(source) = checked {
                        let wire =
                            source.observe().map_err(|error| ActualProviderError::Callback(error.to_string()))?;
                        vec![values.heap().import_strict(ara_session::native_clone_wire(&wire))?]
                    } else {
                        convert_one(&graph, root, interrupted_next)?
                    };
                    if fragment.iter().any(|member| member.same_identity(root)) {
                        alias_version_generation = Some(generation);
                    }
                    self.actual_cache.lock().unwrap().insert(
                        identity,
                        ActualMemo {
                            generation,
                            alias_version_generation,
                            root: root.clone(),
                            interrupted_next,
                            route: route.clone(),
                            fragment: fragment.clone(),
                        },
                    );
                    fragment
                }
            };
            output.extend(fragment);
        }
        self.entry_ids.lock().unwrap().retain(|identity, _| identity.upgrade().is_some());
        Ok(output)
    }
}

fn convert_one(
    graph: &HostGraph<'_>,
    source: &ValueHandle,
    interrupted_next: bool,
) -> Result<Vec<ValueHandle>, ActualProviderError> {
    let role = graph.string(&graph.get(source, "role")?)?;
    if role.equals_ascii("assistant") {
        let aborted = graph.is(source, "stopReason", "aborted")?;
        let interrupted = if aborted {
            let error_id = graph.get(source, "errorId")?;
            let bits = match graph.0.heap().snapshot(&error_id)? {
                ValueSnapshot::Number(value) if value.is_finite() => value.trunc().rem_euclid(4_294_967_296.0) as u32,
                _ => 0,
            };
            bits & 0x0400_0000 != 0 || graph.is(source, "errorMessage", "Interrupted by user")?
        } else {
            false
        };
        let mut derived = source.clone();
        if interrupted_next || interrupted {
            let content = graph.get(source, "content")?;
            let length = graph.0.heap().array_length(&content, graph.0.runtime())?;
            let mut end = length;
            while end > 0 {
                let block = graph.get(&content, &(end - 1).to_string())?;
                if !graph.is(&block, "type", "text")? || !trimmed_empty(&graph.string(&graph.get(&block, "text")?)?) {
                    break;
                }
                end -= 1;
            }
            let mut start = end;
            while start > 0 {
                let block = graph.get(&content, &(start - 1).to_string())?;
                if !graph.is(&block, "type", "thinking")?
                    || trimmed_empty(&graph.string(&graph.get(&block, "thinking")?)?)
                    || graph.0.heap().truthy(&graph.get(&block, "thinkingSignature")?)?
                {
                    break;
                }
                start -= 1;
            }
            if start != end {
                derived = graph.spread(source)?;
                let stripped = graph.0.heap().array_slice(&content, 0.0, Some(f64::from(start)), graph.0.runtime())?;
                graph.put(&derived, "content", stripped)?;
            }
            if interrupted
                && !interrupted_next
                && graph.0.heap().array_length(&graph.get(&derived, "content")?, graph.0.runtime())? == 0
            {
                return Ok(Vec::new());
            }
        }
        return Ok(vec![derived]);
    }
    if role.equals_ascii("user") || role.equals_ascii("developer") {
        let attribution = graph.get(source, "attribution")?;
        let derived = graph.spread(source)?;
        if graph.nullish(&attribution)? {
            graph.put(
                &derived,
                "attribution",
                graph.text(if role.equals_ascii("user") { "user" } else { "agent" })?,
            )?;
        }
        return Ok(vec![derived]);
    }
    if role.equals_ascii("toolResult") {
        let source_content = graph.get(source, "content")?;
        let pruned = graph.get(source, "prunedAt")?;
        let content = if matches!(graph.0.heap().snapshot(&pruned)?, ValueSnapshot::Undefined) {
            source_content
        } else {
            let blocks = graph.members(&source_content)?;
            let mut texts = Vec::new();
            let mut first_text = None;
            for (index, block) in blocks.iter().enumerate() {
                if graph.is(block, "type", "text")? {
                    texts.push(graph.string(&graph.get(block, "text")?)?);
                    first_text.get_or_insert(index);
                }
            }
            let mut text = join(&texts, "");
            if text.units().is_empty() {
                text = "[Output truncated]".into();
            }
            let mut out = Vec::new();
            if first_text.is_none() {
                out.push(graph.block(text.clone())?);
            }
            for (index, block) in blocks.into_iter().enumerate() {
                if !graph.is(&block, "type", "text")? {
                    out.push(block);
                } else if first_text == Some(index) {
                    out.push(graph.block(text.clone())?);
                }
            }
            graph.array(out)?
        };
        let derived = graph.spread(source)?;
        graph.put(&derived, "content", content)?;
        let attribution = graph.get(source, "attribution")?;
        graph.put(
            &derived,
            "attribution",
            if graph.nullish(&attribution)? { graph.text("agent")? } else { attribution },
        )?;
        return Ok(vec![derived]);
    }
    if role.equals_ascii("custom") || role.equals_ascii("hookMessage") {
        return custom(graph, source);
    }
    if role.equals_ascii("fileMention") {
        return file_mention(graph, source);
    }
    if role.equals_ascii("branchSummary") || role.equals_ascii("compactionSummary") {
        return summary(graph, source, role.equals_ascii("branchSummary"));
    }
    if role.equals_ascii("bashExecution") || role.equals_ascii("pythonExecution") {
        if graph.0.heap().truthy(&graph.get(source, "excludeFromContext")?)? {
            return Ok(Vec::new());
        }
        // OutputMeta includes artifact/truncation/diagnostic consumers. Do not
        // silently omit it or substitute a serde copy in this source packet.
        if graph.0.heap().truthy(&graph.get(source, "meta")?)? {
            return Err(ActualProviderError::Unsupported("actual execution-message OutputMeta notice"));
        }
        let python = role.equals_ascii("pythonExecution");
        let input = graph
            .0
            .heap()
            .to_string(&graph.get(source, if python { "code" } else { "command" })?, graph.0.runtime())?;
        let mut parts = if python {
            vec!["Ran Python:\n```python\n".into(), input, "\n```\n".into()]
        } else {
            vec!["Ran `".into(), input, "`\n".into()]
        };
        let output = graph.get(source, "output")?;
        if graph.0.heap().truthy(&output)? {
            parts.extend([
                if python { "Output:\n```\n" } else { "```\n" }.into(),
                graph.0.heap().to_string(&output, graph.0.runtime())?,
                "\n```".into(),
            ]);
        } else {
            parts.push("(no output)".into());
        }
        if graph.0.heap().truthy(&graph.get(source, "cancelled")?)? {
            parts.push(if python { "\n\n(execution cancelled)" } else { "\n\n(command cancelled)" }.into());
        } else {
            let exit = graph.get(source, "exitCode")?;
            if !graph.nullish(&exit)?
                && !matches!(graph.0.heap().snapshot(&exit)?, ValueSnapshot::Number(value) if value == 0.0)
            {
                parts.extend([
                    if python { "\n\nExecution failed with code " } else { "\n\nCommand exited with code " }.into(),
                    graph.0.heap().to_string(&exit, graph.0.runtime())?,
                ]);
            }
        }
        let message = graph.message("user", graph.array([graph.block(join(&parts, ""))?])?, source)?;
        graph.put(&message, "attribution", graph.text("user")?)?;
        return Ok(vec![message]);
    }
    Ok(Vec::new())
}

fn custom(graph: &HostGraph<'_>, source: &ValueHandle) -> Result<Vec<ValueHandle>, ActualProviderError> {
    let content = graph.get(source, "content")?;
    let kind = graph.0.heap().snapshot(&content)?;
    if !matches!(kind, ValueSnapshot::String(_) | ValueSnapshot::Array) {
        return Ok(Vec::new());
    }
    let skill = graph.is(source, "role", "custom")?
        && graph.is(source, "customType", "skill-prompt")?
        && graph.is(source, "attribution", "user")?;
    let steering = graph.is(source, "role", "custom")?
        && graph.is(source, "customType", "collab-prompt")?
        && graph.is(source, "attribution", "user")?;
    if steering {
        let (text, images) = match &kind {
            ValueSnapshot::String(text) => (text.clone(), Vec::new()),
            _ => {
                let mut texts = Vec::new();
                let mut images = Vec::new();
                for block in graph.members(&content)? {
                    if graph.is(&block, "type", "text")? {
                        texts.push(graph.string(&graph.get(&block, "text")?)?);
                    } else if graph.is(&block, "type", "image")? {
                        images.push(block);
                    }
                }
                (join(&texts, "\n"), images)
            }
        };
        let converted = if text.units().is_empty() {
            content
        } else {
            let text = template(include_str!("actual_host_prompts/user-interjection.md"), &text, "message");
            if matches!(kind, ValueSnapshot::String(_)) {
                graph.text(text)?
            } else {
                let mut blocks = vec![graph.block(text)?];
                blocks.extend(images);
                graph.array(blocks)?
            }
        };
        let message = graph.message("user", converted, source)?;
        graph.put(&message, "attribution", graph.text("user")?)?;
        return Ok(vec![message]);
    }
    let blocks = if let ValueSnapshot::String(text) = kind { graph.array([graph.block(text)?])? } else { content };
    if skill {
        let message = graph.message("user", blocks, source)?;
        graph.put(&message, "attribution", graph.text("user")?)?;
        return Ok(vec![message]);
    }
    let mut text = Vec::new();
    let mut images = Vec::new();
    for block in graph.members(&blocks)? {
        if graph.is(&block, "type", "text")? {
            text.push(block);
        } else if graph.is(&block, "type", "image")? {
            images.push(block);
        }
    }
    if images.is_empty() {
        return Ok(vec![graph.message("developer", blocks, source)?]);
    }
    let mut output = Vec::new();
    if !text.is_empty() {
        output.push(graph.message("developer", graph.array(text)?, source)?);
    }
    let custom_type = graph.0.heap().to_string(&graph.get(source, "customType")?, graph.0.runtime())?;
    let notice = join(&["Images attached to ".into(), custom_type, ".".into()], "");
    let mut blocks = vec![graph.block(notice)?];
    blocks.extend(images);
    output.push(graph.message("user", graph.array(blocks)?, source)?);
    Ok(output)
}

fn file_mention(graph: &HostGraph<'_>, source: &ValueHandle) -> Result<Vec<ValueHandle>, ActualProviderError> {
    let mut text = Vec::new();
    let mut image_text = Vec::new();
    let mut images = Vec::new();
    for file in graph.members(&graph.get(source, "files")?)? {
        let image = graph.get(&file, "image")?;
        let path = graph.0.heap().to_string(&graph.get(&file, "path")?, graph.0.runtime())?;
        let content = graph.get(&file, "content")?;
        let mut wrapped = vec!["<file path=\"".into(), path, "\">\n".into()];
        if graph.0.heap().truthy(&content)? {
            wrapped.extend([graph.0.heap().to_string(&content, graph.0.runtime())?, "\n".into()]);
        }
        wrapped.push("</file>".into());
        if graph.0.heap().truthy(&image)? {
            image_text.push(join(&wrapped, ""));
            images.push(image);
        } else {
            text.push(join(&wrapped, ""));
        }
    }
    let mut output = Vec::new();
    if !text.is_empty() {
        output.push(graph.message("developer", graph.array([graph.block(join(&text, "\n"))?])?, source)?);
    }
    if !image_text.is_empty() {
        let mut blocks = vec![graph.block(join(&image_text, "\n"))?];
        blocks.extend(images);
        output.push(graph.message("user", graph.array(blocks)?, source)?);
    }
    for message in &output {
        graph.put(message, "attribution", graph.text("user")?)?;
    }
    Ok(output)
}

fn template(source: &str, value: &WireString, name: &str) -> WireString {
    let marker = format!("{{{{{name}}}}}");
    let parts = source.split(&marker).map(WireString::from).collect::<Vec<_>>();
    let mut units = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            units.extend_from_slice(value.units());
        }
        units.extend_from_slice(part.units());
    }
    WireString::from_units(units)
}
fn summary(graph: &HostGraph<'_>, source: &ValueHandle, branch: bool) -> Result<Vec<ValueHandle>, ActualProviderError> {
    // Native route proofs/archive retrieval belong to the original Session
    // selection owner. This packet does not invent a second proof/transcript.
    let preserved = graph.get(source, "preserveData")?;
    if !graph.nullish(&preserved)? {
        return Err(ActualProviderError::Unsupported("actual checked native summary/archive projection"));
    }
    let text = graph.string(&graph.get(source, "summary")?)?;
    let mut content = Vec::new();
    if branch {
        content.push(graph.block(template(
            include_str!("actual_host_prompts/branch-summary-context.md"),
            &text,
            "summary",
        ))?);
    } else {
        let blocks = graph.get(source, "blocks")?;
        if !matches!(graph.0.heap().snapshot(&blocks)?, ValueSnapshot::Undefined) {
            content.push(graph.block(text)?);
            content.extend(graph.members(&blocks)?);
        } else {
            let prompt = if graph.is(source, "method", "handoff")? {
                include_str!("actual_host_prompts/handoff-summary-context.md")
            } else {
                include_str!("actual_host_prompts/compaction-summary-context.md")
            };
            content.push(graph.block(template(prompt, &text, "summary"))?);
            let images = graph.get(source, "images")?;
            if !graph.nullish(&images)? {
                content.extend(graph.members(&images)?);
            }
        }
    }
    let message = graph.message("user", graph.array(content)?, source)?;
    graph.put(&message, "attribution", graph.text("agent")?)?;
    graph.put(&message, "historyRewriteAt", graph.get(source, "timestamp")?)?;
    if !branch {
        graph.put(&message, "providerPayload", graph.get(source, "providerPayload")?)?;
    }
    Ok(vec![message])
}
