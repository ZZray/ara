//! Actual payload handling for the existing stream loop. No receiver loop,
//! transcript, dispatch engine, or canonical copy lives here.
use super::actual_prepare::{PreparedActualBatch, prepare_staged_actual_calls};
use super::*;
use crate::agent_message::{CurrentArgumentsSlot, StagedGraphPreparation};
use crate::tool::{ActualToolBinding, ActualToolSource};
use ara_ai::ActualAssistantEvent;
use ara_ai::validation::{
    GraphContextValidation, normalize_graph_optional_nulls_for_schema, validate_graph_json_context,
};
use ara_rpc::{
    WireString,
    value_heap::{
        CompiledFunctionForm, CompiledFunctionSpec, PropertyDescriptor, ValueError, ValueHandle,
        ValueHeap, ValueRuntime, ValueSnapshot,
    },
};

pub(super) struct ActualFinalAssistant {
    pub message: AgentMessage,
    pub values: Arc<SessionValues>,
    // This retains wire advertisement; it does not replace Host tool authority.
    #[allow(dead_code)] // Captured wire advertisement is not execution authority.
    pub post_hook_tools: Option<Arc<[ValueHandle]>>,
    pub original_post_hook_tools: Option<ValueHandle>,
    pub actual_tool_source: Option<Arc<dyn ActualToolSource>>,
    // Full prepared receipts survive final publication on this same owner.
    pub prepared_dispatch: Option<PreparedActualBatch>,
    pub reason: Option<StopReason>,
    pub failure: Option<Arc<ActualProviderError>>,
}

pub(super) struct ActualStreamState {
    pub values: Arc<SessionValues>,
    post_hook_tools: Option<Arc<[ValueHandle]>>,
    original_post_hook_tools: Option<ValueHandle>,
    actual_tool_source: Option<Arc<dyn ActualToolSource>>,
    owner: Option<AgentMessage>,
    published: bool,
    completed: HashSet<WireString>,
    arg_streams: crate::arg_stream::ArgStreams,
}

fn combine(primary: &mut Option<Arc<ActualProviderError>>, error: impl Into<ActualProviderError>) {
    let secondary = Arc::new(error.into());
    *primary = Some(match primary.take() {
        Some(primary) => Arc::new(ActualProviderError::Compound { primary, secondary }),
        None => secondary,
    });
}

fn own_data(
    values: &SessionValues,
    target: &ValueHandle,
    key: &str,
) -> Result<ValueHandle, ActualProviderError> {
    match values
        .heap()
        .own_descriptor(target, &key.into(), values.runtime())?
    {
        Some(PropertyDescriptor::Data { value, .. }) => Ok(value),
        _ => Err(ActualProviderError::Unsupported(
            "stream execution controls require own data properties",
        )),
    }
}
fn optional_data(
    values: &SessionValues,
    target: &ValueHandle,
    key: &str,
) -> Result<Option<ValueHandle>, ActualProviderError> {
    match values
        .heap()
        .own_descriptor(target, &key.into(), values.runtime())?
    {
        Some(PropertyDescriptor::Data { value, .. }) => Ok(Some(value)),
        None => Ok(None),
        _ => Err(ActualProviderError::Unsupported(
            "stream retention metadata accessor",
        )),
    }
}
fn text(values: &SessionValues, value: &ValueHandle) -> Result<WireString, ActualProviderError> {
    match values.heap().snapshot(value)? {
        ValueSnapshot::String(text) => Ok(text),
        _ => Err(ActualProviderError::Unsupported(
            "stream execution control must be Text",
        )),
    }
}
fn put(
    values: &SessionValues,
    target: &ValueHandle,
    key: &str,
    value: ValueHandle,
) -> Result<(), ActualProviderError> {
    values
        .heap()
        .define_own_data(target, key.into(), value, values.runtime())?;
    Ok(())
}
fn plain_content(
    values: &SessionValues,
    root: &ValueHandle,
) -> Result<Vec<ValueHandle>, ActualProviderError> {
    values.check_handle(root)?;
    if !matches!(values.heap().snapshot(root)?, ValueSnapshot::Object) {
        return Err(ActualProviderError::Unsupported(
            "stream root must be an ordinary object",
        ));
    }
    let content = own_data(values, root, "content")?;
    if !matches!(values.heap().snapshot(&content)?, ValueSnapshot::Array) {
        return Err(ActualProviderError::Unsupported(
            "stream content must be an ordinary array",
        ));
    }
    let length = values.heap().array_length(&content, values.runtime())?;
    let mut members = Vec::new();
    for index in 0..length {
        let member = own_data(values, &content, &index.to_string())?;
        if !matches!(values.heap().snapshot(&member)?, ValueSnapshot::Object) {
            return Err(ActualProviderError::Unsupported(
                "stream content requires ordinary object members",
            ));
        }
        members.push(member);
    }
    Ok(members)
}
fn is_tool(values: &SessionValues, member: &ValueHandle) -> Result<bool, ActualProviderError> {
    Ok(text(values, &own_data(values, member, "type")?)? == WireString::from("toolCall"))
}

/// Bounded ordinary-data retention. Argument/member handles are aliased, never
/// exported/reimported. Custom array filter/iterator capabilities remain R2-B.
fn retain_completed(
    values: &SessionValues,
    root: ValueHandle,
    completed: &HashSet<WireString>,
) -> Result<ValueHandle, ActualProviderError> {
    let reason = text(values, &own_data(values, &root, "stopReason")?)?;
    if reason != WireString::from("error") && reason != WireString::from("aborted") {
        return Ok(root);
    }
    let members = plain_content(values, &root)?;
    let mut retained = Vec::new();
    for member in &members {
        if !is_tool(values, member)?
            || completed.contains(&text(values, &own_data(values, member, "id")?)?)
        {
            retained.push(member.clone());
        }
    }
    if retained.len() == members.len() {
        return Ok(root);
    }
    let content = values.heap().array_with_length(0)?;
    for (index, member) in retained.into_iter().enumerate() {
        put(values, &content, &index.to_string(), member)?;
    }
    let replacement = values.heap().spread(&root, values.runtime())?;
    put(values, &replacement, "content", content)?;
    let prior = optional_data(values, &root, "stopDetails")?;
    let already = match &prior {
        Some(prior) if matches!(values.heap().snapshot(prior)?, ValueSnapshot::Object) => {
            optional_data(values, prior, "type")?
                .map(|value| text(values, &value))
                .transpose()?
                == Some(WireString::from(
                    STREAM_INTERRUPTED_AFTER_CONTENT_STOP_DETAIL,
                ))
        }
        Some(prior)
            if !matches!(
                values.heap().snapshot(prior)?,
                ValueSnapshot::Null | ValueSnapshot::Undefined
            ) =>
        {
            return Err(ActualProviderError::Unsupported(
                "stream retention stopDetails shape",
            ));
        }
        _ => false,
    };
    if !already {
        let details = values.heap().object()?;
        put(
            values,
            &details,
            "type",
            values
                .heap()
                .string(STREAM_INTERRUPTED_AFTER_CONTENT_STOP_DETAIL.into())?,
        )?;
        let category = match &prior {
            Some(prior) if matches!(values.heap().snapshot(prior)?, ValueSnapshot::Object) => {
                optional_data(values, prior, "type")?
            }
            _ => None,
        };
        let explanation = match &prior {
            Some(prior) if matches!(values.heap().snapshot(prior)?, ValueSnapshot::Object) => {
                optional_data(values, prior, "explanation")?
            }
            _ => None,
        };
        let explanation = match explanation {
            Some(value)
                if !matches!(
                    values.heap().snapshot(&value)?,
                    ValueSnapshot::Null | ValueSnapshot::Undefined
                ) =>
            {
                Some(value)
            }
            _ => optional_data(values, &root, "errorMessage")?,
        };
        put(
            values,
            &details,
            "category",
            category.unwrap_or(values.heap().null()?),
        )?;
        put(
            values,
            &details,
            "explanation",
            explanation.unwrap_or(values.heap().null()?),
        )?;
        put(values, &replacement, "stopDetails", details)?;
    }
    Ok(replacement)
}

impl ActualStreamState {
    pub fn new(
        values: Arc<SessionValues>,
        post_hook_tools: Option<Arc<[ValueHandle]>>,
        actual_tool_source: Option<Arc<dyn ActualToolSource>>,
        original_post_hook_tools: Option<ValueHandle>,
    ) -> Self {
        Self {
            values,
            post_hook_tools,
            original_post_hook_tools,
            actual_tool_source,
            owner: None,
            published: false,
            completed: HashSet::new(),
            arg_streams: crate::arg_stream::ArgStreams::new(),
        }
    }

    pub fn preview_wake(&self) -> Arc<tokio::sync::Notify> {
        self.arg_streams.preview_wake()
    }

    pub async fn flush_previews(&mut self, sink: &dyn AgentEventSink) {
        self.arg_streams.flush(sink).await;
    }

    async fn stage(
        &mut self,
        context: &mut Vec<AgentMessage>,
        sink: &dyn AgentEventSink,
        root: ValueHandle,
        publish: bool,
    ) -> Result<(), ActualProviderError> {
        if let Some(owner) = &self.owner {
            owner.replace_stream_graph(root, &self.values)?;
        } else {
            let owner = AgentMessage::from_stream_graph(root, &self.values)?;
            self.owner = Some(owner.clone());
            if publish {
                context.push(owner.clone());
                self.published = true;
                sink.emit(AgentEvent::MessageStart { message: owner }).await;
            }
        }
        Ok(())
    }

    pub async fn update(
        &mut self,
        context: &mut Vec<AgentMessage>,
        sink: &dyn AgentEventSink,
        event: ActualAssistantEvent,
    ) -> Result<(), ActualProviderError> {
        self.values.check_handle(event.partial())?;
        if let Some(index) = event.content_index() {
            self.values.check_handle(index)?;
        }
        match &event {
            AssistantMessageEvent::ToolcallDelta { delta, .. } => {
                self.values.check_handle(delta)?
            }
            AssistantMessageEvent::ImageEnd { content, .. } => self.values.check_handle(content)?,
            AssistantMessageEvent::ToolcallEnd { tool_call, .. } => {
                self.values.check_handle(tool_call)?;
                // Responses publishes a new authoritative tool object while
                // updating arguments on the retained block. The fixed Agent
                // tracks its ID; it does not require member object identity.
                self.completed.insert(text(
                    &self.values,
                    &own_data(&self.values, tool_call, "id")?,
                )?);
            }
            _ => {}
        }
        let start = matches!(event, AssistantMessageEvent::Start { .. });
        if start {
            self.completed.clear();
        }
        if !start && self.owner.is_none() {
            return Ok(());
        }
        let values = &self.values;
        let source = self.actual_tool_source.as_deref();
        self.arg_streams
            .on_actual_event(&event, values, |call| match source {
                Some(source) => resolve_actual_tool_for_call(source, call, values),
                None => Ok(None),
            })?;
        self.arg_streams.flush(sink).await;
        let had_owner = self.owner.is_some();
        self.stage(context, sink, event.partial().clone(), true)
            .await?;
        if had_owner {
            sink.emit(AgentEvent::ActualMessageUpdate {
                message: self.owner.as_ref().expect("staged owner").clone(),
                event,
            })
            .await;
        }
        Ok(())
    }

    /// Called from the original loop's terminal, EOF, cancellation and producer
    /// failure branches. One final pin follows transform and prepare observation.
    #[allow(clippy::too_many_arguments)]
    pub async fn finish(
        mut self,
        context: &mut Vec<AgentMessage>,
        config: &AdmittedExecution,
        ctl: &RunControl,
        sink: &dyn AgentEventSink,
        root: Option<ValueHandle>,
        failure: Option<ActualProviderError>,
        transform: bool,
        error_event: bool,
    ) -> ActualFinalAssistant {
        let mut failure = failure.map(Arc::new);
        if let Some(root) = root {
            match self.stage(context, sink, root.clone(), false).await {
                Ok(()) => {
                    if error_event
                        && let Err(error) = self
                            .owner
                            .as_ref()
                            .expect("terminal staged one owner")
                            .remember_stream_terminal_origin(root, &self.values)
                    {
                        combine(&mut failure, error);
                    }
                }
                Err(error) => combine(&mut failure, error),
            }
        }
        if self.owner.is_none() {
            // A synthetic rejection owns no upstream Graph data. This typed
            // metadata import creates an empty failure root; no received root,
            // argument, name or effect is projected through serde.
            let mut empty = AssistantMessage::empty(
                &config.model.api,
                &config.model.provider,
                &config.model.id,
            );
            empty.stop_reason = if ctl.is_cancelled() {
                StopReason::Aborted
            } else {
                StopReason::Error
            };
            empty.error_message = Some(if ctl.is_cancelled() {
                ctl.abort_text()
            } else {
                failure
                    .as_ref()
                    .map_or(MISSING_TERMINAL_EVENT.into(), ToString::to_string)
            });
            let source = serde_json::to_value(Message::Assistant(empty.clone()))
                .expect("synthetic typed metadata serializes");
            let graph = self
                .values
                .heap()
                .import_serde(&source)
                .map_err(ActualProviderError::from);
            match graph {
                Ok(graph) => {
                    if let Err(error) = self.stage(context, sink, graph, false).await {
                        combine(&mut failure, error);
                    }
                }
                Err(error) => combine(&mut failure, error),
            }
            if self.owner.is_none() {
                let owner = AgentMessage::from_model(Message::Assistant(empty));
                context.push(owner.clone());
                sink.emit(AgentEvent::MessageStart {
                    message: owner.clone(),
                })
                .await;
                sink.emit(AgentEvent::MessageEnd {
                    message: owner.clone(),
                })
                .await;
                self.arg_streams.finish(sink).await;
                return ActualFinalAssistant {
                    message: owner,
                    values: self.values,
                    post_hook_tools: self.post_hook_tools,
                    original_post_hook_tools: self.original_post_hook_tools,
                    actual_tool_source: self.actual_tool_source,
                    prepared_dispatch: None,
                    reason: None,
                    failure,
                };
            }
        }
        let owner = self
            .owner
            .take()
            .expect("failure or stream staged one owner");
        let current = || match owner.body_snapshot() {
            AgentMessageBody::Graph(root) => {
                self.values.check_handle(&root)?;
                Ok(root)
            }
            _ => Err(ActualProviderError::Unsupported(
                "actual assistant transform replaced its Graph domain",
            )),
        };
        // Abort/EOF changes the shallow root, retaining original content handles.
        if ctl.is_cancelled() || (!transform && failure.is_some()) {
            let stopped = (|| {
                let root = self
                    .values
                    .heap()
                    .spread(&current()?, self.values.runtime())?;
                put(
                    &self.values,
                    &root,
                    "stopReason",
                    self.values.heap().string(if ctl.is_cancelled() {
                        "aborted".into()
                    } else {
                        "error".into()
                    })?,
                )?;
                let reason = if ctl.is_cancelled() {
                    ctl.abort_text()
                } else {
                    failure.as_ref().expect("failed EOF/producer").to_string()
                };
                put(
                    &self.values,
                    &root,
                    "errorMessage",
                    self.values.heap().string(reason.as_str().into())?,
                )?;
                owner.replace_stream_graph(root, &self.values)?;
                Ok::<_, ActualProviderError>(())
            })();
            if let Err(error) = stopped {
                combine(&mut failure, error);
            }
        }
        match current().and_then(|root| retain_completed(&self.values, root, &self.completed)) {
            Ok(root) => {
                if let Err(error) = owner.replace_stream_graph(root, &self.values) {
                    combine(&mut failure, error);
                }
            }
            Err(error) => combine(&mut failure, error),
        }
        if transform && failure.is_none() && !ctl.is_cancelled() {
            let callback = std::panic::AssertUnwindSafe(config.hooks.transform_assistant_graph(
                &owner,
                &ctl.token,
                &self.values,
            ))
            .catch_unwind();
            let result = tokio::select! {
                biased;
                _ = ctl.token.cancelled() => Err(ActualProviderError::Callback(ctl.abort_text())),
                result = callback => match result {
                    Ok(result) => result,
                    Err(panic) => Err(ActualProviderError::Callback(format!("Actual assistant transform panicked: {}", panic_text(panic)))),
                },
            };
            if let Err(error) = result {
                combine(&mut failure, error);
            }
        }
        // A transform can race the external signal. Keep that original cause
        // in both the Run receipt and the same shallow final message.
        if ctl.is_cancelled() {
            let aborted = (|| {
                let root = self
                    .values
                    .heap()
                    .spread(&current()?, self.values.runtime())?;
                put(
                    &self.values,
                    &root,
                    "stopReason",
                    self.values.heap().string("aborted".into())?,
                )?;
                put(
                    &self.values,
                    &root,
                    "errorMessage",
                    self.values
                        .heap()
                        .string(ctl.abort_text().as_str().into())?,
                )?;
                let root = retain_completed(&self.values, root, &self.completed)?;
                owner.replace_stream_graph(root, &self.values)?;
                Ok::<_, ActualProviderError>(())
            })();
            if let Err(error) = aborted {
                combine(&mut failure, error);
            }
        }
        if ctl.is_cancelled()
            && let Some(reason) = ctl.scoped_abort()
        {
            let labelled = (|| {
                let root = self
                    .values
                    .heap()
                    .spread(&current()?, self.values.runtime())?;
                let labels = self.values.heap().object()?;
                let mut has_tool = false;
                for member in plain_content(&self.values, &root)? {
                    if !is_tool(&self.values, &member)? {
                        continue;
                    }
                    let id = self.values.heap().get(&member, &"id".into(), self.values.runtime())?;
                    let id = self.values.heap().to_string(&id, self.values.runtime())?;
                    self.values.heap().define_own_data(
                        &labels,
                        id.clone().into(),
                        self.values.heap().string(reason.for_tool(&id).into())?,
                        self.values.runtime(),
                    )?;
                    has_tool = true;
                }
                if has_tool {
                    put(&self.values, &root, "toolCallAbortMessages", labels)?;
                }
                owner.replace_stream_graph(root, &self.values)?;
                Ok::<_, ActualProviderError>(())
            })();
            if let Err(error) = labelled {
                combine(&mut failure, error);
            }
        }
        let mut observed_calls = false;
        match current().and_then(|root| plain_content(&self.values, &root)) {
            Ok(members) => {
                for member in members {
                    match is_tool(&self.values, &member) {
                        Ok(true) => {
                            observed_calls = true;
                            break;
                        }
                        Ok(false) => {}
                        Err(error) => {
                            combine(&mut failure, error);
                            break;
                        }
                    }
                }
            }
            Err(error) => combine(&mut failure, error),
        }
        // Only normal terminal completion gains a local preparation profile.
        // Abort/EOF/producer failure retain the Arc without new preparation reads.
        let staged_preparation = if transform && failure.is_none() && !ctl.is_cancelled() {
            match owner.capture_staged_graph_preparation(&self.values) {
                Ok(prepared) => Some(prepared),
                Err(error) => {
                    combine(&mut failure, error);
                    None
                }
            }
        } else {
            None
        };
        let mut prepared_dispatch = None;
        if observed_calls && let Some(staged) = &staged_preparation {
            match prepare_staged_actual_calls(
                &owner,
                staged,
                self.actual_tool_source.as_deref(),
                context,
                config,
                ctl,
                &self.values,
            )
            .await
            {
                Ok(prepared) => prepared_dispatch = Some(prepared),
                Err(error) => combine(&mut failure, error),
            }
        }
        let mut reason = None;
        let mut finalization_error = None;
        match current().and_then(|root| match &staged_preparation {
            Some(prepared) => {
                let (reason, calls, slots) =
                    owner.install_prepared_final_graph(root, &self.values, prepared)?;
                if let Some(batch) = &mut prepared_dispatch {
                    batch.bind_owned(&slots)?;
                }
                Ok((reason, calls))
            }
            None => owner
                .install_final_graph(root, &self.values)
                .map_err(ActualProviderError::from),
        }) {
            Ok((final_reason, has_calls)) => {
                reason = Some(final_reason);
                if has_calls && !observed_calls {
                    combine(
                        &mut failure,
                        ActualProviderError::Protocol(
                            "final actual controls added an unprepared call",
                        ),
                    );
                }
                if error_event && !matches!(final_reason, StopReason::Error | StopReason::Aborted) {
                    combine(
                        &mut failure,
                        ActualProviderError::Protocol("error event has nonfailure final controls"),
                    );
                }
            }
            Err(error) => {
                finalization_error = Some(Arc::new(error.clone()));
                combine(&mut failure, error);
            }
        }
        if !self.published {
            context.push(owner.clone());
            sink.emit(AgentEvent::MessageStart {
                message: owner.clone(),
            })
            .await;
        }
        if let Some(finalization_error) = finalization_error {
            sink.emit(AgentEvent::ActualMessageEnd {
                message: owner.clone(),
                finalization_error,
            })
            .await;
        } else {
            sink.emit(AgentEvent::MessageEnd {
                message: owner.clone(),
            })
            .await;
        }
        self.arg_streams.finish(sink).await;
        ActualFinalAssistant {
            message: owner,
            values: self.values,
            post_hook_tools: self.post_hook_tools,
            original_post_hook_tools: self.original_post_hook_tools,
            actual_tool_source: self.actual_tool_source,
            prepared_dispatch,
            reason,
            failure,
        }
    }
}

/// Returns after emitting TurnEnd; the original outer loop owns AgentEnd and
/// dequeue/follow-up behavior. No synthetic tool result or dispatch is produced.
#[allow(clippy::too_many_arguments)]
pub(super) async fn emit_actual_turn_end(
    context: &mut Vec<AgentMessage>,
    new_messages: &mut Vec<AgentMessage>,
    mut final_assistant: ActualFinalAssistant,
    config: &AgentConfig,
    ctl: &RunControl,
    sink: &dyn AgentEventSink,
    tool_results: Vec<AgentMessage>,
    will_continue: bool,
) -> Result<Option<RunEnd>, Arc<ActualProviderError>> {
    let end = ctl.gate_external_end().or_else(|| {
        if final_assistant.failure.is_some() {
            return Some(RunEnd::Error);
        }
        match final_assistant.reason {
            Some(StopReason::Stop | StopReason::Length | StopReason::ToolUse) => None,
            Some(StopReason::Aborted) => Some(RunEnd::Aborted),
            Some(StopReason::Error) => Some(RunEnd::Error),
            _ => Some(RunEnd::Error),
        }
    });
    sink.emit(AgentEvent::ActualTurnEnd {
        message: final_assistant.message.clone(),
        tool_results: tool_results.clone(),
        preparation_error: final_assistant.failure.clone(),
    })
    .await;
    let hook_allowed = final_assistant.failure.is_none()
        && !matches!(
            final_assistant.reason,
            Some(StopReason::Error | StopReason::Aborted)
        )
        && (!ctl.is_cancelled() || ctl.terminal_result());
    if hook_allowed {
        for owner in std::iter::once(&final_assistant.message).chain(&tool_results) {
            if let Err(error) = owner.graph_identity_controls() {
                combine(
                    &mut final_assistant.failure,
                    ActualProviderError::from(error),
                );
            }
        }
        if let Some(error) = final_assistant.failure.take() {
            return Err(error);
        }
        let cancel = ctl.turn_end_token();
        let stop_cause = ctl.stop_cause();
        let callback = std::panic::AssertUnwindSafe(config.hooks.turn_end_actual_context(
            ActualTurnEndContext {
                assistant_message: &final_assistant.message,
                tool_results: &tool_results,
                context: ActualAgentContextView {
                    system_prompt: &config.system_prompt,
                    messages: context,
                    tool_source: final_assistant.actual_tool_source.as_deref(),
                },
                will_continue,
                stop_reason: final_assistant.reason,
                stop_cause: stop_cause.as_ref(),
                values: &final_assistant.values,
            },
            &cancel,
        ))
        .catch_unwind();
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => Ok(Err(ActualProviderError::Callback(ctl.abort_text()))),
            result = callback => result,
        };
        let inputs = match result {
            Ok(Ok(inputs)) => inputs,
            Ok(Err(error)) => {
                combine(&mut final_assistant.failure, error);
                Vec::new()
            }
            Err(panic) => {
                combine(
                    &mut final_assistant.failure,
                    ActualProviderError::Callback(format!(
                        "Actual turn-end hook panicked: {}",
                        panic_text(panic)
                    )),
                );
                Vec::new()
            }
        };
        // Keep Native content/details/extension mutations. Recheck only the
        // already pinned Owned identity controls, also on callback failure.
        for owner in std::iter::once(&final_assistant.message).chain(&tool_results) {
            if let Err(error) = owner.graph_identity_controls() {
                combine(
                    &mut final_assistant.failure,
                    ActualProviderError::from(error),
                );
            }
        }
        if let Some(error) = final_assistant.failure.take() {
            return Err(error);
        }
        // Returned inputs were consumed by the hook. As with the typed path,
        // commit them even if cancellation raced its return.
        for input in inputs {
            context.push(input.clone());
            new_messages.push(input.clone());
            emit_raw_input(sink, input).await;
        }
        return Ok(ctl.gate_external_end());
    }
    Ok(ctl.gate_external_end().or(end))
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn process_actual_turn(
    context: &mut Vec<AgentMessage>,
    new_messages: &mut Vec<AgentMessage>,
    mut final_assistant: ActualFinalAssistant,
    config: &AgentConfig,
    ctl: &RunControl,
    sink: &dyn AgentEventSink,
    soft_state: &SoftToolRequirementState,
    soft_requirement: Option<&crate::soft_tool_requirement::SoftToolRequirement>,
    static_choice: Option<&ToolChoice>,
    paused_turn_continuations: &mut usize,
) -> Result<(Option<RunEnd>, bool), Arc<ActualProviderError>> {
    use super::actual_execute::{
        ActualExecutionResult, actual_call_members, actual_pause_turn, actual_soft_rejection,
        execute_actual_calls, synthetic_actual_calls,
    };
    let calls = match actual_call_members(&final_assistant) {
        Ok(calls) => calls,
        Err(error) => {
            combine(&mut final_assistant.failure, error);
            Vec::new()
        }
    };
    let deadline_hit = deadline_passed(config);
    let runnable = matches!(
        final_assistant.reason,
        Some(StopReason::Stop | StopReason::ToolUse)
    );
    let mut has_more =
        runnable && !calls.is_empty() && !deadline_hit && final_assistant.failure.is_none();
    let result = if matches!(
        final_assistant.reason,
        Some(StopReason::Error | StopReason::Aborted)
    ) || final_assistant.failure.is_some()
    {
        let reason = if final_assistant.reason == Some(StopReason::Aborted) {
            "aborted"
        } else {
            "error"
        };
        synthetic_actual_calls(&final_assistant, &calls, reason, None, sink).await
    } else {
        let rejection = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            actual_soft_rejection(
                &final_assistant,
                &calls,
                soft_state,
                soft_requirement,
                static_choice,
                config,
            )
        }));
        let rejection = match rejection {
            Ok(result) => result,
            Err(panic) => Err(ActualProviderError::Callback(format!(
                "Actual soft-tool predicate panicked: {}",
                panic_text(panic)
            ))),
        };
        match rejection {
            Err(error) => {
                has_more = false;
                ActualExecutionResult {
                    results: Vec::new(),
                    failure: Some(Arc::new(error)),
                }
            }
            Ok(Some(reason)) => {
                let results = synthetic_actual_calls(
                    &final_assistant,
                    &calls,
                    "skipped",
                    Some(&reason),
                    sink,
                )
                .await;
                if results.failure.is_none() {
                    soft_state.record_escalation(
                        &soft_requirement
                            .expect("active actual soft rejection")
                            .tool_name,
                    );
                    has_more = true;
                }
                results
            }
            Ok(None) if has_more => {
                execute_actual_calls(&final_assistant, context, config, ctl, sink).await
            }
            Ok(None) if !calls.is_empty() => {
                let (reason, error) = if deadline_hit {
                    ("aborted", Some(DEADLINE_TEXT))
                } else if final_assistant.reason == Some(StopReason::Length) {
                    ("length", None)
                } else {
                    ("skipped", None)
                };
                let results =
                    synthetic_actual_calls(&final_assistant, &calls, reason, error, sink).await;
                if final_assistant.reason == Some(StopReason::Length)
                    && !results.results.is_empty()
                    && !deadline_hit
                {
                    has_more = true;
                }
                results
            }
            Ok(None) => ActualExecutionResult {
                results: Vec::new(),
                failure: None,
            },
        }
    };
    for message in &result.results {
        context.push(message.clone());
        new_messages.push(message.clone());
    }
    if let Some(error) = result.failure {
        combine(&mut final_assistant.failure, error.as_ref().clone());
        has_more = false;
    }
    if !calls.is_empty() {
        *paused_turn_continuations = 0;
    } else if !has_more
        && final_assistant.failure.is_none()
        && final_assistant.reason == Some(StopReason::Stop)
        && *paused_turn_continuations < 8
    {
        match actual_pause_turn(&final_assistant) {
            Ok(true) => {
                *paused_turn_continuations += 1;
                has_more = true;
            }
            Ok(false) => {}
            Err(error) => combine(&mut final_assistant.failure, error),
        }
    }
    let end = emit_actual_turn_end(
        context,
        new_messages,
        final_assistant,
        config,
        ctl,
        sink,
        result.results,
        has_more && !ctl.is_cancelled(),
    )
    .await?;
    Ok((end, has_more))
}

pub(super) async fn prepare_owned_actual_tail(
    owner: AgentMessage,
    context: &[AgentMessage],
    config: &AdmittedExecution,
    ctl: &RunControl,
) -> Result<ActualFinalAssistant, ActualProviderError> {
    let values = config
        .provider_context
        .as_ref()
        .and_then(|binding| binding.actual_values.clone())
        .ok_or(ActualProviderError::Unsupported(
            "known-never-executed actual tail has no original Session values",
        ))?;
    let mut failure = None;
    let source = match config.hooks.actual_tool_source(&values) {
        Ok(source) => source,
        Err(error) => {
            combine(&mut failure, error);
            None
        }
    };
    let transformed = std::panic::AssertUnwindSafe(
        config
            .hooks
            .transform_assistant_graph(&owner, &ctl.token, &values),
    )
    .catch_unwind()
    .await;
    match transformed {
        Ok(Ok(())) => {}
        Ok(Err(error)) => combine(&mut failure, error),
        Err(panic) => combine(
            &mut failure,
            ActualProviderError::Callback(format!(
                "Actual tail transform panicked: {}",
                panic_text(panic)
            )),
        ),
    }
    let staged = match owner.capture_owned_graph_preparation(&values) {
        Ok(staged) => Some(staged),
        Err(error) => {
            combine(&mut failure, error);
            None
        }
    };
    let mut prepared_dispatch = None;
    if failure.is_none()
        && let Some(staged) = &staged
    {
        match prepare_staged_actual_calls(
            &owner,
            staged,
            source.as_deref(),
            context,
            config,
            ctl,
            &values,
        )
        .await
        {
            Ok(prepared) => prepared_dispatch = Some(prepared),
            Err(error) => combine(&mut failure, error),
        }
    }
    let reason = match owner.body_snapshot() {
        AgentMessageBody::Graph(root) => own_data(&values, &root, "stopReason")
            .and_then(|value| text(&values, &value))
            .and_then(|text| {
                [
                    StopReason::Stop,
                    StopReason::Length,
                    StopReason::ToolUse,
                    StopReason::Error,
                    StopReason::Aborted,
                ]
                .into_iter()
                .find(|reason| text == WireString::from(reason.as_str()))
                .ok_or(ActualProviderError::Protocol(
                    "owned actual tail has unknown stopReason",
                ))
            }),
        _ => Err(ActualProviderError::Protocol(
            "owned actual tail changed Graph domain",
        )),
    };
    let reason = match reason {
        Ok(reason) => Some(reason),
        Err(error) => {
            combine(&mut failure, error);
            None
        }
    };
    Ok(ActualFinalAssistant {
        message: owner,
        values,
        post_hook_tools: None,
        original_post_hook_tools: None,
        actual_tool_source: source,
        prepared_dispatch,
        reason,
        failure,
    })
}

/// Fixed Native resolveToolForCall (agent-loop.ts:2254-2268). The current Host
/// collection is acquired once; both searches use real Get(find)/Call and the
/// original call handle, preserving method overrides and predicate Get effects.
static TOOL_NAME_PREDICATE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "omp:596f2da7101178214aa27a753529d15e6b7ad91d:agent-loop.ts:2264",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 1,
    body: actual_tool_name_predicate,
};
static TOOL_ALIAS_PREDICATE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "omp:596f2da7101178214aa27a753529d15e6b7ad91d:agent-loop.ts:2265",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 1,
    body: actual_tool_alias_predicate,
};
fn actual_tool_name_predicate(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    arguments: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    actual_tool_predicate(heap, captures, arguments, runtime, false)
}
fn actual_tool_alias_predicate(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    arguments: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    actual_tool_predicate(heap, captures, arguments, runtime, true)
}
fn actual_tool_predicate(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    arguments: &[ValueHandle],
    runtime: &dyn ValueRuntime,
    alias: bool,
) -> Result<ValueHandle, ValueError> {
    let tool = match arguments.first() {
        Some(tool) => tool.clone(),
        None => heap.undefined()?,
    };
    if alias {
        let name = heap.get(&tool, &"customWireName".into(), runtime)?;
        if matches!(heap.snapshot(&name)?, ValueSnapshot::Undefined) {
            return heap.boolean(false);
        }
    }
    let key = if alias { "customWireName" } else { "name" };
    let selected = heap.get(&tool, &key.into(), runtime)?;
    let called = heap.get(&captures[0], &"name".into(), runtime)?;
    let equal = match (heap.snapshot(&selected)?, heap.snapshot(&called)?) {
        (ValueSnapshot::Number(left), ValueSnapshot::Number(right)) => left == right,
        _ => heap.object_is(&selected, &called)?,
    };
    heap.boolean(equal)
}

pub(super) fn resolve_actual_tool_for_call(
    source: &dyn ActualToolSource,
    call: &ValueHandle,
    values: &SessionValues,
) -> Result<Option<ActualToolBinding>, ActualProviderError> {
    values.check_handle(call)?;
    let heap = values.heap();
    let tools = source.current_tools(values)?;
    if let Some(tools) = tools.as_ref() {
        values.check_handle(tools)?;
        if !matches!(
            heap.snapshot(tools)?,
            ValueSnapshot::Null | ValueSnapshot::Undefined
        ) {
            for alias in [false, true] {
                let find = heap.get(tools, &"find".into(), values.runtime())?;
                let spec = if alias {
                    &TOOL_ALIAS_PREDICATE
                } else {
                    &TOOL_NAME_PREDICATE
                };
                let predicate = heap.fresh_compiled_function(spec, std::slice::from_ref(call))?;
                let selected = heap.call(&find, tools, &[predicate], values.runtime())?;
                if !matches!(
                    heap.snapshot(&selected)?,
                    ValueSnapshot::Null | ValueSnapshot::Undefined
                ) {
                    values.check_handle(&selected)?;
                    let binding = source.bind_selected(&selected, values)?;
                    binding.check_selected(&selected, values)?;
                    return Ok(Some(binding));
                }
            }
        }
    }
    // Native optional call does not evaluate arguments when the Host
    // fallback callback is absent. Presence is not an execution permission.
    if !source.has_fallback() {
        return Ok(None);
    }
    // Native reaches this Get only after both searches miss. Preserve its
    // original value for the actual Host fallback; never perform ToString.
    let name = heap.get(call, &"name".into(), values.runtime())?;
    values.check_handle(&name)?;
    let binding = source.resolve_fallback(&name, values)?;
    if let Some(binding) = &binding {
        binding.check_lineage(values)?;
    }
    Ok(binding)
}

/// A transient preparation receipt for an already owned call. This is not a
/// ready-to-run tool capability and does not install/pin a staging root.
#[derive(Clone, Debug)]
#[allow(dead_code)] // Actual dispatch requires the remaining R2-B driver/hooks.
pub(super) struct GraphArgumentsPreparation {
    pub raw_arguments: ValueHandle,
    pub normalized_arguments: ValueHandle,
    pub changed: bool,
    pub slot: CurrentArgumentsSlot,
    pub validation: GraphContextValidation,
}

/// Caller supplies the upgraded JSON schema, rather than treating post-hook
/// Tool.parameters as an implicitly upgraded schema or rebuilding typed tools.
#[allow(dead_code)] // Separate explicit schema-upgrade consumer.
pub(super) fn prepare_owned_graph_arguments(
    owner: &AgentMessage,
    member: &ValueHandle,
    values: &SessionValues,
    upgraded_json_schema: &ValueHandle,
) -> Result<GraphArgumentsPreparation, ActualProviderError> {
    values.check_handle(upgraded_json_schema)?;
    values.check_handle(member)?;
    prepare_graph_arguments_from_slot(
        owner.current_arguments_slot(member)?,
        values,
        upgraded_json_schema,
    )
}

/// Staged and Owned consumers share one once-read, both-outcome readmission
/// engine. Supplied parameters are not implicitly an upgraded JSON schema.
#[allow(dead_code)] // Full R2-B driver/hooks remain mandatory before dispatch.
pub(super) fn prepare_staged_graph_arguments(
    staged: &StagedGraphPreparation,
    member: &ValueHandle,
    values: &SessionValues,
    upgraded_json_schema: &ValueHandle,
) -> Result<GraphArgumentsPreparation, ActualProviderError> {
    values.check_handle(member)?;
    prepare_graph_arguments_from_slot(
        staged.current_arguments_slot(member)?,
        values,
        upgraded_json_schema,
    )
}

fn prepare_graph_arguments_from_slot(
    slot: CurrentArgumentsSlot,
    values: &SessionValues,
    upgraded_json_schema: &ValueHandle,
) -> Result<GraphArgumentsPreparation, ActualProviderError> {
    let prepared = (|| {
        values.check_handle(upgraded_json_schema)?;
        let raw = slot.read(values.runtime())?;
        values.check_handle(&raw)?;
        let normalized = normalize_graph_optional_nulls_for_schema(
            values.factory(),
            upgraded_json_schema,
            &raw,
            true,
        )?;
        let validation =
            validate_graph_json_context(values.factory(), upgraded_json_schema, &normalized.value)?;
        Ok::<_, ActualProviderError>((raw, normalized, validation))
    })();
    // The same recheck also runs after read/normalizer/checker failure. It never
    // performs a second Get(arguments); both failure receipts remain visible.
    let admission = slot.recheck_current().map_err(ActualProviderError::from);
    match (prepared, admission) {
        (Ok((raw, normalized, validation)), Ok(())) => Ok(GraphArgumentsPreparation {
            raw_arguments: raw,
            normalized_arguments: normalized.value,
            changed: normalized.changed,
            slot,
            validation,
        }),
        (Err(primary), Err(secondary)) => Err(ActualProviderError::Compound {
            primary: Arc::new(primary),
            secondary: Arc::new(secondary),
        }),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
    }
}

impl GraphArgumentsPreparation {
    #[allow(dead_code)] // Fresh capability binding does not make a call executable.
    pub(super) fn bind_owned_slot(
        mut self,
        slot: CurrentArgumentsSlot,
    ) -> Result<Self, ActualProviderError> {
        self.slot.require_owned_rebinding(&slot)?;
        self.slot = slot;
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_message::GraphOwnerError;
    use ara_rpc::value_heap::{
        CallableTarget, CapabilityId, NoRuntime, RuntimeReply, RuntimeRequest, ValueError,
        ValueHeap, ValueRuntime,
    };
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    fn own(heap: &Arc<ValueHeap>, object: &ValueHandle, key: &str, value: ValueHandle) {
        heap.define_own_data(object, key.into(), value, &NoRuntime)
            .unwrap();
    }
    fn getter(heap: &Arc<ValueHeap>, object: &ValueHandle, key: &str, capability: u64) {
        heap.define_own(
            object,
            key.into(),
            PropertyDescriptor::Accessor {
                get: Some(CallableTarget::Host(CapabilityId(capability))),
                set: None,
                enumerable: true,
                configurable: true,
            },
            &NoRuntime,
        )
        .unwrap();
    }
    fn root(heap: &Arc<ValueHeap>, member: ValueHandle) -> ValueHandle {
        let root = heap.object().unwrap();
        own(
            heap,
            &root,
            "role",
            heap.string("assistant".into()).unwrap(),
        );
        own(heap, &root, "content", heap.array(vec![member]).unwrap());
        own(
            heap,
            &root,
            "stopReason",
            heap.string("toolUse".into()).unwrap(),
        );
        root
    }
    #[derive(Clone, Copy, Debug)]
    enum Stage {
        Read,
        Normalize,
        Checker,
    }
    #[derive(Clone, Copy, Debug)]
    enum Mutation {
        None,
        Cache,
        Remove,
        ReplaceRoot,
        Id,
        Name,
        Role,
        Stop,
    }
    #[derive(Clone, Copy, Debug)]
    enum Failure {
        None,
        Native,
        Host,
        Foreign,
    }
    struct Runtime {
        heap: Arc<ValueHeap>,
        owner: AgentMessage,
        root: ValueHandle,
        member: ValueHandle,
        raw: ValueHandle,
        thrown: ValueHandle,
        foreign: ValueHandle,
        stage: Stage,
        mutation: Mutation,
        failure: Failure,
        reads: AtomicUsize,
        effects: AtomicUsize,
    }
    impl Runtime {
        fn mutate(&self) {
            // A callback can safely reenter the same canonical record.
            assert!(matches!(
                self.owner.body_snapshot(),
                AgentMessageBody::Graph(_)
            ));
            match self.mutation {
                Mutation::None => {}
                Mutation::Cache => self.owner.invalidate_derived(),
                Mutation::Remove => own(
                    &self.heap,
                    &self.root,
                    "content",
                    self.heap.array(Vec::new()).unwrap(),
                ),
                Mutation::ReplaceRoot => {
                    self.owner
                        .replace_body(AgentMessageBody::Graph(root(
                            &self.heap,
                            self.member.clone(),
                        )))
                        .unwrap();
                }
                Mutation::Id => own(
                    &self.heap,
                    &self.member,
                    "id",
                    self.heap.string("changed".into()).unwrap(),
                ),
                Mutation::Name => own(
                    &self.heap,
                    &self.member,
                    "name",
                    self.heap.string("changed".into()).unwrap(),
                ),
                Mutation::Role => own(
                    &self.heap,
                    &self.root,
                    "role",
                    self.heap.string("user".into()).unwrap(),
                ),
                Mutation::Stop => own(
                    &self.heap,
                    &self.root,
                    "stopReason",
                    self.heap.string("error".into()).unwrap(),
                ),
            }
        }
        fn effect(&self, success: ValueHandle) -> Result<RuntimeReply, ValueError> {
            self.effects.fetch_add(1, Ordering::SeqCst);
            self.mutate();
            match self.failure {
                Failure::None => Ok(RuntimeReply::Value(success)),
                Failure::Native => Err(ValueError::Thrown(self.thrown.clone())),
                Failure::Host => Err(ValueError::ExpiredCapability(CapabilityId(777))),
                Failure::Foreign => Ok(RuntimeReply::Value(self.foreign.clone())),
            }
        }
    }
    impl ValueRuntime for Runtime {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            match request.capability().0 {
                80 => {
                    self.reads.fetch_add(1, Ordering::SeqCst);
                    if matches!(self.stage, Stage::Read) {
                        self.effect(self.raw.clone())
                    } else {
                        Ok(RuntimeReply::Value(self.raw.clone()))
                    }
                }
                81 => self.effect(self.heap.undefined()?),
                82 => self.effect(self.raw.clone()),
                _ => Err(ValueError::MissingCapability(request.capability())),
            }
        }
    }
    fn keeps_thrown(error: &ActualProviderError, thrown: &ValueHandle) -> bool {
        match error {
            ActualProviderError::Value(ValueError::Thrown(value))
            | ActualProviderError::Owner(GraphOwnerError::Native(ValueError::Thrown(value))) => {
                value.same_identity(thrown)
            }
            ActualProviderError::Owner(GraphOwnerError::Admission {
                callback_failure: Some(ValueError::Thrown(value)),
                ..
            }) => value.same_identity(thrown),
            ActualProviderError::Compound { primary, secondary } => {
                keeps_thrown(primary, thrown) || keeps_thrown(secondary, thrown)
            }
            _ => false,
        }
    }
    fn has_owner_failure(error: &ActualProviderError) -> bool {
        match error {
            ActualProviderError::Owner(GraphOwnerError::Admission { .. }) => true,
            ActualProviderError::Compound { primary, secondary } => {
                has_owner_failure(primary) || has_owner_failure(secondary)
            }
            _ => false,
        }
    }

    fn plain_call(heap: &Arc<ValueHeap>, id: &str) -> ValueHandle {
        let call = heap.object().unwrap();
        for (key, value) in [("type", "toolCall"), ("id", id), ("name", "echo")] {
            own(heap, &call, key, heap.string(value.into()).unwrap());
        }
        own(heap, &call, "arguments", heap.object().unwrap());
        call
    }

    fn observe_staged_final_profile_and_binding() {
        let heap = ValueHeap::new().unwrap();
        let raw = heap
            .import_serde(&serde_json::json!({"optional":null,"text":"kept"}))
            .unwrap();
        let member = plain_call(&heap, "call");
        getter(&heap, &member, "arguments", 80);
        let graph_root = root(&heap, member.clone());
        let capture_values = SessionValues::new(heap.clone(), Arc::new(NoRuntime)).unwrap();
        let owner = AgentMessage::from_stream_graph(graph_root.clone(), &capture_values).unwrap();
        let runtime = Arc::new(Runtime {
            heap: heap.clone(),
            owner: owner.clone(),
            root: graph_root.clone(),
            member: member.clone(),
            raw: raw.clone(),
            thrown: heap.symbol(None).unwrap(),
            foreign: ValueHeap::new().unwrap().object().unwrap(),
            stage: Stage::Read,
            mutation: Mutation::None,
            failure: Failure::None,
            reads: AtomicUsize::new(0),
            effects: AtomicUsize::new(0),
        });
        let values = Arc::new(SessionValues::new(heap.clone(), runtime.clone()).unwrap());
        let wire_tools: Arc<[ValueHandle]> = vec![heap.object().unwrap()].into();
        let retained = ActualStreamState::new(
            values.clone(),
            Some(wire_tools.clone()),
            None,
            Some(heap.array(vec![heap.object().unwrap()]).unwrap()),
        );
        assert!(Arc::ptr_eq(
            retained.post_hook_tools.as_ref().unwrap(),
            &wire_tools
        ));
        assert!(
            ActualStreamState::new(values.clone(), None, None, None)
                .post_hook_tools
                .is_none()
        );
        let staged = owner.capture_staged_graph_preparation(&values).unwrap();
        assert!(
            owner.current_arguments_slot(&member).is_err(),
            "capture must not pin early"
        );
        assert_eq!(runtime.reads.load(Ordering::SeqCst), 0);
        owner.invalidate_derived();
        let staged_slot = staged.current_arguments_slot(&member).unwrap();
        staged_slot.recheck_current().unwrap();
        let schema = heap.import_serde(&serde_json::json!({"type":"object","properties":{"optional":{"type":"string"},"text":{"type":"string"}},"required":["text"]})).unwrap();
        let prepared = prepare_staged_graph_arguments(&staged, &member, &values, &schema).unwrap();
        assert!(prepared.changed && prepared.raw_arguments.same_identity(&raw));
        let normalized = prepared.normalized_arguments.clone();
        assert_eq!(runtime.reads.load(Ordering::SeqCst), 1);
        let (reason, has_calls, slots) = owner
            .install_prepared_final_graph(graph_root.clone(), &values, &staged)
            .unwrap();
        assert!(matches!(reason, StopReason::ToolUse) && has_calls && slots.len() == 1);
        assert!(
            staged_slot.recheck_current().is_err(),
            "staged capability expires at final pin"
        );
        assert!(
            owner
                .install_prepared_final_graph(graph_root.clone(), &values, &staged)
                .is_err(),
            "pin only once"
        );
        assert!(
            prepared
                .clone()
                .bind_owned_slot(owner.current_arguments_slot(&member).unwrap())
                .is_err(),
            "a generic Owned slot must not substitute for the matched final-pin receipt"
        );
        let rebound = prepared.bind_owned_slot(slots[0].clone()).unwrap();
        assert!(
            rebound.raw_arguments.same_identity(&raw)
                && rebound.normalized_arguments.same_identity(&normalized)
        );
        assert_eq!(
            runtime.reads.load(Ordering::SeqCst),
            1,
            "final pin and fresh binding must not Get arguments"
        );
        own(
            &heap,
            &graph_root,
            "stopReason",
            heap.string("stop".into()).unwrap(),
        );
        rebound.slot.recheck_current().unwrap(); // Preserve existing Owned runnable compatibility.
        own(
            &heap,
            &member,
            "name",
            heap.string("changed".into()).unwrap(),
        );
        assert!(rebound.slot.recheck_current().is_err());
        assert_eq!(runtime.reads.load(Ordering::SeqCst), 1);

        // Exact terminal Text applies during staging even within one Owned class.
        let member = plain_call(&heap, "exact");
        let graph_root = root(&heap, member.clone());
        let owner = AgentMessage::from_stream_graph(graph_root.clone(), &values).unwrap();
        let staged = owner.capture_staged_graph_preparation(&values).unwrap();
        let slot = staged.current_arguments_slot(&member).unwrap();
        own(
            &heap,
            &graph_root,
            "stopReason",
            heap.string("stop".into()).unwrap(),
        );
        assert!(slot.recheck_current().is_err());
        assert!(
            owner
                .install_prepared_final_graph(graph_root.clone(), &values, &staged)
                .is_err()
        );
        assert!(owner.current_arguments_slot(&member).is_err());

        // Per-slot admission checks its selected identity and all scalar order,
        // while final capture additionally compares EVERY selected identity.
        let first = plain_call(&heap, "first");
        let second = plain_call(&heap, "second");
        let graph_root = root(&heap, first.clone());
        let content = heap
            .get(&graph_root, &"content".into(), &NoRuntime)
            .unwrap();
        own(&heap, &content, "1", second.clone());
        let owner = AgentMessage::from_stream_graph(graph_root.clone(), &values).unwrap();
        let staged = owner.capture_staged_graph_preparation(&values).unwrap();
        let first_slot = staged.current_arguments_slot(&first).unwrap();
        own(&heap, &content, "1", plain_call(&heap, "second"));
        first_slot.recheck_current().unwrap(); // Equal controls of another member can pass here.
        assert!(
            staged
                .current_arguments_slot(&second)
                .unwrap()
                .recheck_current()
                .is_err()
        );
        assert!(
            owner
                .install_prepared_final_graph(graph_root.clone(), &values, &staged)
                .is_err()
        );
        assert!(
            owner.current_arguments_slot(&first).is_err(),
            "mismatch must leave owner unpinned"
        );

        // The expected profile also binds owner and root, not only scalar content.
        let other_owner = AgentMessage::from_stream_graph(graph_root.clone(), &values).unwrap();
        assert!(
            other_owner
                .install_prepared_final_graph(graph_root.clone(), &values, &staged)
                .is_err()
        );
        let replacement = root(&heap, first.clone());
        assert!(
            owner
                .install_prepared_final_graph(replacement.clone(), &values, &staged)
                .is_err()
        );
        owner.replace_stream_graph(replacement, &values).unwrap();
        assert!(first_slot.recheck_current().is_err());
        assert!(
            owner
                .install_prepared_final_graph(graph_root, &values, &staged)
                .is_err()
        );

        // Zero-call capture includes stopReason unconditionally and rejects
        // both a later inserted call and a terminal change with no calls.
        for inserted in [false, true] {
            let graph_root = heap
                .import_serde(
                    &serde_json::json!({"role":"assistant","content":[],"stopReason":"stop"}),
                )
                .unwrap();
            let owner = AgentMessage::from_stream_graph(graph_root.clone(), &values).unwrap();
            let staged = owner.capture_staged_graph_preparation(&values).unwrap();
            if inserted {
                let content = heap
                    .get(&graph_root, &"content".into(), &NoRuntime)
                    .unwrap();
                own(&heap, &content, "0", plain_call(&heap, "inserted"));
            } else {
                own(
                    &heap,
                    &graph_root,
                    "stopReason",
                    heap.string("length".into()).unwrap(),
                );
            }
            assert!(
                owner
                    .install_prepared_final_graph(graph_root, &values, &staged)
                    .is_err()
            );
        }
        let graph_root = heap
            .import_serde(&serde_json::json!({"role":"assistant","content":[],"stopReason":"stop"}))
            .unwrap();
        let owner = AgentMessage::from_stream_graph(graph_root.clone(), &values).unwrap();
        let staged = owner.capture_staged_graph_preparation(&values).unwrap();
        owner.invalidate_derived();
        let (reason, calls, slots) = owner
            .install_prepared_final_graph(graph_root, &values, &staged)
            .unwrap();
        assert!(matches!(reason, StopReason::Stop) && !calls && slots.is_empty());
    }

    struct BindingTool;
    #[async_trait]
    impl AgentTool for BindingTool {
        fn definition(&self) -> ara_ai::Tool {
            panic!("binding must not read typed definition")
        }
        async fn execute(
            &self,
            _: &str,
            _: ara_ai::JsonObject,
            _: CancellationToken,
            _: crate::tool::UpdateFn,
        ) -> Result<ToolOutput, crate::tool::ToolError> {
            panic!("selection must not execute")
        }
    }

    struct SourceFixture {
        collection: Mutex<Option<ValueHandle>>,
        capability: Mutex<Arc<dyn AgentTool>>,
        wrong_source: Mutex<Option<ValueHandle>>,
        fallback: Mutex<Option<ActualToolBinding>>,
        fallback_present: AtomicBool,
        last_fallback_name: Mutex<Option<ValueHandle>>,
        trace: Arc<Mutex<Vec<&'static str>>>,
    }
    impl ActualToolSource for SourceFixture {
        fn current_tools(
            &self,
            _: &SessionValues,
        ) -> Result<Option<ValueHandle>, ActualProviderError> {
            self.trace.lock().unwrap().push("tools");
            Ok(self.collection.lock().unwrap().clone())
        }
        fn bind_selected(
            &self,
            selected: &ValueHandle,
            _: &SessionValues,
        ) -> Result<ActualToolBinding, ActualProviderError> {
            self.trace.lock().unwrap().push("bind");
            Ok(ActualToolBinding {
                source: self
                    .wrong_source
                    .lock()
                    .unwrap()
                    .clone()
                    .unwrap_or_else(|| selected.clone()),
                capability: self.capability.lock().unwrap().clone(),
            })
        }
        fn has_fallback(&self) -> bool {
            self.fallback_present.load(Ordering::SeqCst)
        }
        fn resolve_fallback(
            &self,
            name: &ValueHandle,
            values: &SessionValues,
        ) -> Result<Option<ActualToolBinding>, ActualProviderError> {
            self.trace.lock().unwrap().push("fallback");
            values.check_handle(name)?;
            *self.last_fallback_name.lock().unwrap() = Some(name.clone());
            Ok(self.fallback.lock().unwrap().clone())
        }
    }
    struct DefaultFallbackSource;
    impl ActualToolSource for DefaultFallbackSource {
        fn current_tools(
            &self,
            _: &SessionValues,
        ) -> Result<Option<ValueHandle>, ActualProviderError> {
            Ok(None)
        }
        fn bind_selected(
            &self,
            _: &ValueHandle,
            _: &SessionValues,
        ) -> Result<ActualToolBinding, ActualProviderError> {
            panic!("no selected source")
        }
    }
    struct SelectionRuntime {
        heap: Arc<ValueHeap>,
        trace: Arc<Mutex<Vec<&'static str>>>,
        name: Mutex<ValueHandle>,
        thrown: ValueHandle,
        override_member: ValueHandle,
        alias_values: Mutex<Vec<ValueHandle>>,
        alias_reads: AtomicUsize,
        retained_predicates: Mutex<Vec<ValueHandle>>,
    }
    impl ValueRuntime for SelectionRuntime {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            let value = match request.capability().0 {
                910 => {
                    self.trace.lock().unwrap().push("call-name");
                    self.name.lock().unwrap().clone()
                }
                911 => {
                    self.trace.lock().unwrap().push("name-zero");
                    self.heap.string("other".into())?
                }
                912 => {
                    self.trace.lock().unwrap().push("name-one");
                    self.heap.string("echo".into())?
                }
                913 => {
                    self.trace.lock().unwrap().push("alias");
                    let index = self.alias_reads.fetch_add(1, Ordering::SeqCst);
                    let values = self.alias_values.lock().unwrap();
                    values
                        .get(index)
                        .or_else(|| values.last())
                        .cloned()
                        .ok_or(ValueError::InvalidNode)?
                }
                914 => {
                    self.trace.lock().unwrap().push("find-get");
                    self.heap
                        .get(&self.heap.array_prototype(), &"find".into(), &NoRuntime)?
                }
                916 => {
                    self.trace.lock().unwrap().push("thrown-name");
                    return Err(ValueError::Thrown(self.thrown.clone()));
                }
                _ => return Err(ValueError::MissingCapability(request.capability())),
            };
            Ok(RuntimeReply::Value(value))
        }
        fn resolve_call(
            &self,
            capability: CapabilityId,
            _: &ValueHandle,
            arguments: &[ValueHandle],
            runtime: &dyn ValueRuntime,
        ) -> Result<RuntimeReply, ValueError> {
            if capability.0 != 915 {
                return self.resolve(RuntimeRequest::Call {
                    capability,
                    receiver: &self.heap.undefined()?,
                    arguments,
                });
            }
            self.trace.lock().unwrap().push("override-find");
            self.retained_predicates
                .lock()
                .unwrap()
                .push(arguments[0].clone());
            let selected = self.heap.call(
                &arguments[0],
                &self.heap.undefined()?,
                std::slice::from_ref(&self.override_member),
                runtime,
            )?;
            Ok(RuntimeReply::Value(if self.heap.truthy(&selected)? {
                self.override_member.clone()
            } else {
                self.heap.undefined()?
            }))
        }
    }

    fn observe_actual_host_source_resolver() {
        let heap = ValueHeap::new().unwrap();
        let trace = Arc::new(Mutex::new(Vec::new()));
        let first = plain_call(&heap, "first");
        let second = plain_call(&heap, "second");
        let source_parameters = heap.object().unwrap();
        own(&heap, &second, "parameters", source_parameters.clone());
        getter(&heap, &first, "name", 911);
        getter(&heap, &second, "name", 912);
        getter(&heap, &first, "customWireName", 913);
        let tools = heap.array(vec![first.clone(), second.clone()]).unwrap();
        getter(&heap, &tools, "find", 914);
        let call = heap.object().unwrap();
        getter(&heap, &call, "name", 910);
        let thrown = heap.symbol(Some("source getter receipt".into())).unwrap();
        let runtime = Arc::new(SelectionRuntime {
            heap: heap.clone(),
            trace: trace.clone(),
            name: Mutex::new(heap.string("echo".into()).unwrap()),
            thrown: thrown.clone(),
            override_member: second.clone(),
            alias_values: Mutex::new(vec![
                heap.string("first differs".into()).unwrap(),
                heap.string("echo".into()).unwrap(),
            ]),
            alias_reads: AtomicUsize::new(0),
            retained_predicates: Mutex::new(Vec::new()),
        });
        let values = Arc::new(SessionValues::new(heap.clone(), runtime.clone()).unwrap());
        let capability: Arc<dyn AgentTool> = Arc::new(BindingTool);
        let source = Arc::new(SourceFixture {
            collection: Mutex::new(Some(tools.clone())),
            capability: Mutex::new(capability.clone()),
            wrong_source: Mutex::new(None),
            fallback: Mutex::new(None),
            fallback_present: AtomicBool::new(true),
            last_fallback_name: Mutex::new(None),
            trace: trace.clone(),
        });
        let retained_source: Arc<dyn ActualToolSource> = source.clone();
        let retained =
            ActualStreamState::new(values.clone(), None, Some(retained_source.clone()), None);
        assert!(Arc::ptr_eq(
            retained.actual_tool_source.as_ref().unwrap(),
            &retained_source
        ));
        assert!(retained.post_hook_tools.is_none());
        let binding = resolve_actual_tool_for_call(source.as_ref(), &call, &values)
            .unwrap()
            .unwrap();
        assert!(
            binding.source.same_identity(&second) && Arc::ptr_eq(&binding.capability, &capability)
        );
        assert!(
            heap.get(&binding.source, &"parameters".into(), &NoRuntime)
                .unwrap()
                .same_identity(&source_parameters)
        );
        assert_eq!(
            *trace.lock().unwrap(),
            [
                "tools",
                "find-get",
                "name-zero",
                "call-name",
                "name-one",
                "call-name",
                "bind"
            ]
        );
        // Wire mutation is unrelated to this source/selected receipt.
        let wire = heap
            .import_serde(
                &serde_json::json!({"name":"renamed-wire","parameters":{"type":"number"}}),
            )
            .unwrap();
        own(&heap, &wire, "parameters", heap.null().unwrap());
        assert!(binding.source.same_identity(&second));

        // First-pass miss, then two actual customWireName Gets and a reached call.name.
        trace.lock().unwrap().clear();
        own(&heap, &second, "name", heap.string("other".into()).unwrap());
        own(&heap, &first, "customWireName", heap.undefined().unwrap());
        getter(&heap, &second, "customWireName", 913);
        let alias = resolve_actual_tool_for_call(source.as_ref(), &call, &values)
            .unwrap()
            .unwrap();
        assert!(alias.source.same_identity(&second));
        assert_eq!(
            *trace.lock().unwrap(),
            [
                "tools",
                "find-get",
                "name-zero",
                "call-name",
                "call-name",
                "find-get",
                "alias",
                "alias",
                "call-name",
                "bind"
            ]
        );

        // Fallback occurs once only after both misses; an unadvertised source is valid.
        trace.lock().unwrap().clear();
        own(&heap, &second, "customWireName", heap.undefined().unwrap());
        let unadvertised = plain_call(&heap, "side");
        *source.fallback.lock().unwrap() = Some(ActualToolBinding {
            source: unadvertised.clone(),
            capability: capability.clone(),
        });
        let fallback = resolve_actual_tool_for_call(source.as_ref(), &call, &values)
            .unwrap()
            .unwrap();
        assert!(fallback.source.same_identity(&unadvertised));
        assert_eq!(
            *trace.lock().unwrap(),
            [
                "tools",
                "find-get",
                "name-zero",
                "call-name",
                "call-name",
                "find-get",
                "call-name",
                "fallback"
            ]
        );
        for absent in [
            None,
            Some(heap.null().unwrap()),
            Some(heap.undefined().unwrap()),
        ] {
            *source.collection.lock().unwrap() = absent;
            trace.lock().unwrap().clear();
            resolve_actual_tool_for_call(source.as_ref(), &call, &values).unwrap();
            assert_eq!(*trace.lock().unwrap(), ["tools", "call-name", "fallback"]);
        }
        // Absent optional fallback must skip the SAME final call.name Get,
        // including its thrown value. Present retains the reached Get/handle.
        *source.collection.lock().unwrap() = None;
        for capability in [910, 916] {
            getter(&heap, &call, "name", capability);
            source.fallback_present.store(false, Ordering::SeqCst);
            trace.lock().unwrap().clear();
            assert!(
                resolve_actual_tool_for_call(source.as_ref(), &call, &values)
                    .unwrap()
                    .is_none()
            );
            assert_eq!(
                *trace.lock().unwrap(),
                ["tools"],
                "absent fallback must read neither name nor fallback"
            );
            source.fallback_present.store(true, Ordering::SeqCst);
            trace.lock().unwrap().clear();
            let present = resolve_actual_tool_for_call(source.as_ref(), &call, &values);
            if capability == 910 {
                assert!(
                    present
                        .unwrap()
                        .unwrap()
                        .source
                        .same_identity(&unadvertised)
                );
                assert!(
                    source
                        .last_fallback_name
                        .lock()
                        .unwrap()
                        .as_ref()
                        .unwrap()
                        .same_identity(&runtime.name.lock().unwrap())
                );
                assert_eq!(*trace.lock().unwrap(), ["tools", "call-name", "fallback"]);
            } else {
                assert!(
                    matches!(present, Err(ActualProviderError::Value(ValueError::Thrown(value))) if value.same_identity(&thrown))
                );
                assert_eq!(*trace.lock().unwrap(), ["tools", "thrown-name"]);
            }
        }
        getter(&heap, &call, "name", 910);
        assert!(matches!(
            resolve_actual_tool_for_call(&DefaultFallbackSource, &call, &values),
            Err(ActualProviderError::Unsupported(
                "actual Host tool fallback"
            ))
        ));

        // Host method override receives a real retained Function. Its captures
        // and fresh identity survive resolver return, using the later runtime.
        *source.collection.lock().unwrap() = Some(tools.clone());
        own(&heap, &second, "name", heap.string("echo".into()).unwrap());
        own(
            &heap,
            &tools,
            "find",
            heap.function(CapabilityId(915)).unwrap(),
        );
        trace.lock().unwrap().clear();
        let selected = resolve_actual_tool_for_call(source.as_ref(), &call, &values)
            .unwrap()
            .unwrap();
        assert!(selected.source.same_identity(&second));
        assert_eq!(
            *trace.lock().unwrap(),
            ["tools", "override-find", "call-name", "bind"]
        );
        let retained = runtime
            .retained_predicates
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .clone();
        assert!(matches!(
            heap.snapshot(&heap.get(&retained, &"length".into(), &NoRuntime).unwrap())
                .unwrap(),
            ValueSnapshot::Number(1.0)
        ));
        assert!(
            !heap
                .has_own(&retained, &"prototype".into(), &NoRuntime)
                .unwrap()
        );
        assert!(
            heap.truthy(
                &heap
                    .call(
                        &retained,
                        &heap.undefined().unwrap(),
                        std::slice::from_ref(&second),
                        runtime.as_ref()
                    )
                    .unwrap()
            )
            .unwrap()
        );
        *runtime.name.lock().unwrap() = heap.string("later".into()).unwrap();
        assert!(
            !heap
                .truthy(
                    &heap
                        .call(
                            &retained,
                            &heap.undefined().unwrap(),
                            std::slice::from_ref(&second),
                            runtime.as_ref()
                        )
                        .unwrap()
                )
                .unwrap()
        );
        *runtime.name.lock().unwrap() = heap.string("echo".into()).unwrap();

        // Restore standard find. Duplicate logical names preserve first identity;
        // strict equality handles numbers without coercion (NaN never matches).
        own(
            &heap,
            &tools,
            "find",
            heap.get(&heap.array_prototype(), &"find".into(), &NoRuntime)
                .unwrap(),
        );
        own(&heap, &first, "name", heap.string("echo".into()).unwrap());
        let duplicate = resolve_actual_tool_for_call(source.as_ref(), &call, &values)
            .unwrap()
            .unwrap();
        assert!(duplicate.source.same_identity(&first));
        let single = heap.array(vec![first.clone()]).unwrap();
        *source.collection.lock().unwrap() = Some(single);
        for value in [
            heap.number(0.0).unwrap(),
            heap.symbol(Some("strict".into())).unwrap(),
            heap.null().unwrap(),
            heap.undefined().unwrap(),
        ] {
            own(&heap, &first, "name", value.clone());
            *runtime.name.lock().unwrap() =
                if matches!(heap.snapshot(&value).unwrap(), ValueSnapshot::Number(_)) {
                    heap.number(-0.0).unwrap()
                } else {
                    value
                };
            assert!(
                resolve_actual_tool_for_call(source.as_ref(), &call, &values)
                    .unwrap()
                    .unwrap()
                    .source
                    .same_identity(&first)
            );
        }
        *source.fallback.lock().unwrap() = None;
        for (selected_name, called_name) in [
            (
                heap.number(f64::NAN).unwrap(),
                heap.number(f64::NAN).unwrap(),
            ),
            (heap.string("1".into()).unwrap(), heap.number(1.0).unwrap()),
        ] {
            own(&heap, &first, "name", selected_name);
            *runtime.name.lock().unwrap() = called_name.clone();
            assert!(
                resolve_actual_tool_for_call(source.as_ref(), &call, &values)
                    .unwrap()
                    .is_none()
            );
            assert!(
                source
                    .last_fallback_name
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .same_identity(&called_name)
            );
        }
        *runtime.name.lock().unwrap() = heap.string("echo".into()).unwrap();

        // Binding checks reject identity/lineage errors; no definition/execute.
        own(&heap, &first, "name", heap.string("echo".into()).unwrap());
        *source.wrong_source.lock().unwrap() = Some(second.clone());
        assert!(matches!(
            resolve_actual_tool_for_call(source.as_ref(), &call, &values),
            Err(ActualProviderError::Protocol(_))
        ));
        *source.wrong_source.lock().unwrap() = Some(ValueHeap::new().unwrap().object().unwrap());
        assert!(matches!(
            resolve_actual_tool_for_call(source.as_ref(), &call, &values),
            Err(ActualProviderError::Value(ValueError::ForeignHeap))
        ));
        *source.wrong_source.lock().unwrap() = None;
        *source.collection.lock().unwrap() =
            Some(ValueHeap::new().unwrap().array(Vec::new()).unwrap());
        trace.lock().unwrap().clear();
        assert!(matches!(
            resolve_actual_tool_for_call(source.as_ref(), &call, &values),
            Err(ActualProviderError::Value(ValueError::ForeignHeap))
        ));
        assert_eq!(*trace.lock().unwrap(), ["tools"]);
        let foreign_call = ValueHeap::new().unwrap().object().unwrap();
        trace.lock().unwrap().clear();
        assert!(resolve_actual_tool_for_call(source.as_ref(), &foreign_call, &values).is_err());
        assert!(trace.lock().unwrap().is_empty());

        // Current collection can change for a later call; earlier receipt stays bound.
        *source.collection.lock().unwrap() = Some(heap.array(vec![second.clone()]).unwrap());
        let next_capability: Arc<dyn AgentTool> = Arc::new(BindingTool);
        *source.capability.lock().unwrap() = next_capability.clone();
        let next = resolve_actual_tool_for_call(source.as_ref(), &call, &values)
            .unwrap()
            .unwrap();
        assert!(
            Arc::ptr_eq(&next.capability, &next_capability)
                && Arc::ptr_eq(&binding.capability, &capability)
        );

        // Missing method and a visited hole fail explicitly; no Rust iteration substitute.
        let no_find = heap.object().unwrap();
        *source.collection.lock().unwrap() = Some(no_find);
        assert!(matches!(
            resolve_actual_tool_for_call(source.as_ref(), &call, &values),
            Err(ActualProviderError::Value(ValueError::TypeError(_)))
        ));
        let holes = heap.array_with_length(1).unwrap();
        *source.collection.lock().unwrap() = Some(holes);
        trace.lock().unwrap().clear();
        assert!(matches!(
            resolve_actual_tool_for_call(source.as_ref(), &call, &values),
            Err(ActualProviderError::Value(ValueError::TypeError(_)))
        ));
        assert_eq!(*trace.lock().unwrap(), ["tools"]);
        getter(&heap, &second, "name", 916);
        *source.collection.lock().unwrap() = Some(heap.array(vec![second]).unwrap());
        assert!(
            matches!(resolve_actual_tool_for_call(source.as_ref(), &call, &values), Err(ActualProviderError::Value(ValueError::Thrown(value))) if value.same_identity(&thrown))
        );
    }

    /// One complete owned-consumer family; no per-assertion test entrances.
    #[test]
    fn owned_graph_arguments_normalization_and_readmission() {
        super::super::actual_prepare::observe_full_staged_prepare_group();
        observe_actual_host_source_resolver();
        observe_staged_final_profile_and_binding();
        // Received terminal provenance survives the exact Core normalization,
        // while a public same-root constructor/replacement cannot claim it.
        {
            let heap = ValueHeap::new().unwrap();
            let values = SessionValues::new(heap.clone(), Arc::new(NoRuntime)).unwrap();
            let source = heap
                .import_serde(&serde_json::json!({
                    "role":"assistant", "stopReason":"error", "errorMessage":"usage_limit_reached",
                    "content":[{"type":"toolCall","id":"unfinished","name":"echo","arguments":{"p":true}}]
                }))
                .unwrap();
            let owner = AgentMessage::from_stream_graph(source.clone(), &values).unwrap();
            owner
                .remember_stream_terminal_origin(source.clone(), &values)
                .unwrap();
            assert!(
                owner.stream_terminal_origin(&values).is_err(),
                "unfinalized source has no proof"
            );
            let retained = retain_completed(&values, source.clone(), &HashSet::new()).unwrap();
            assert!(!retained.same_identity(&source));
            owner
                .replace_stream_graph(retained.clone(), &values)
                .unwrap();
            owner
                .install_final_graph(retained.clone(), &values)
                .unwrap();
            assert!(
                owner
                    .stream_terminal_origin(&values)
                    .unwrap()
                    .unwrap()
                    .same_identity(&source)
            );
            assert!(
                AgentMessage::from_graph(retained.clone(), None)
                    .unwrap()
                    .stream_terminal_origin(&values)
                    .unwrap()
                    .is_none()
            );
            let foreign =
                SessionValues::new(ValueHeap::new().unwrap(), Arc::new(NoRuntime)).unwrap();
            assert!(owner.stream_terminal_origin(&foreign).is_err());
            owner
                .replace_body(AgentMessageBody::Graph(retained))
                .unwrap();
            assert!(
                owner.stream_terminal_origin(&values).unwrap().is_none(),
                "public replacement revokes origin"
            );
        }
        let heap = ValueHeap::new().unwrap();
        let raw = heap
            .import_serde(&serde_json::json!({"optional":null,"text":"kept"}))
            .unwrap();
        let member = heap.object().unwrap();
        own(
            &heap,
            &member,
            "type",
            heap.string("toolCall".into()).unwrap(),
        );
        own(&heap, &member, "id", heap.string("call".into()).unwrap());
        own(&heap, &member, "name", heap.string("echo".into()).unwrap());
        own(&heap, &member, "arguments", raw.clone());
        let owner = AgentMessage::from_graph(root(&heap, member.clone()), None).unwrap();
        let values = SessionValues::new(heap.clone(), Arc::new(NoRuntime)).unwrap();
        let schema = heap.import_serde(&serde_json::json!({"type":"object","properties":{"optional":{"type":"string"},"text":{"type":"string"}},"required":["text"]})).unwrap();
        let prepared = prepare_owned_graph_arguments(&owner, &member, &values, &schema).unwrap();
        assert!(prepared.changed && prepared.raw_arguments.same_identity(&raw));
        assert!(!prepared.normalized_arguments.same_identity(&raw));
        assert!(
            matches!(prepared.validation, GraphContextValidation::Accept(value) if value.same_identity(&prepared.normalized_arguments))
        );
        assert!(
            prepared.slot.read(&NoRuntime).unwrap().same_identity(&raw),
            "normalization must not assign canonical arguments"
        );
        let reject = heap
            .import_serde(&serde_json::json!({"type":"object","required":["missing"]}))
            .unwrap();
        let prepared = prepare_owned_graph_arguments(&owner, &member, &values, &reject).unwrap();
        assert!(
            !prepared.changed
                && matches!(prepared.validation, GraphContextValidation::Reject(result) if !result.success)
        );
        let foreign = ValueHeap::new().unwrap().object().unwrap();
        assert!(matches!(
            prepare_owned_graph_arguments(&owner, &member, &values, &foreign),
            Err(ActualProviderError::Value(ValueError::ForeignHeap))
        ));

        // Every callback boundary is followed by recheck on success and error;
        // mutation to each owner/root/member control invalidates the receipt.
        for staged_mode in [false, true] {
            for stage in [Stage::Read, Stage::Normalize, Stage::Checker] {
                for mutation in [
                    Mutation::None,
                    Mutation::Cache,
                    Mutation::Remove,
                    Mutation::ReplaceRoot,
                    Mutation::Id,
                    Mutation::Name,
                    Mutation::Role,
                    Mutation::Stop,
                ] {
                    for failure in [
                        Failure::None,
                        Failure::Native,
                        Failure::Host,
                        Failure::Foreign,
                    ] {
                        let heap = ValueHeap::new().unwrap();
                        let raw = heap.object().unwrap();
                        let member = heap.object().unwrap();
                        own(
                            &heap,
                            &member,
                            "type",
                            heap.string("toolCall".into()).unwrap(),
                        );
                        own(&heap, &member, "id", heap.string("call".into()).unwrap());
                        own(&heap, &member, "name", heap.string("echo".into()).unwrap());
                        getter(&heap, &member, "arguments", 80);
                        let graph_root = root(&heap, member.clone());
                        let capture_values =
                            SessionValues::new(heap.clone(), Arc::new(NoRuntime)).unwrap();
                        let owner = if staged_mode {
                            AgentMessage::from_stream_graph(graph_root.clone(), &capture_values)
                                .unwrap()
                        } else {
                            AgentMessage::from_graph(graph_root.clone(), None).unwrap()
                        };
                        let staged = staged_mode.then(|| {
                            owner
                                .capture_staged_graph_preparation(&capture_values)
                                .unwrap()
                        });
                        let thrown = heap.symbol(Some("original exception".into())).unwrap();
                        let runtime = Arc::new(Runtime {
                            heap: heap.clone(),
                            owner: owner.clone(),
                            root: graph_root,
                            member: member.clone(),
                            raw: raw.clone(),
                            thrown: thrown.clone(),
                            foreign: ValueHeap::new().unwrap().object().unwrap(),
                            stage,
                            mutation,
                            failure,
                            reads: AtomicUsize::new(0),
                            effects: AtomicUsize::new(0),
                        });
                        let values = SessionValues::new(heap.clone(), runtime.clone()).unwrap();
                        let schema = heap.object().unwrap();
                        match stage {
                            Stage::Read => {}
                            Stage::Normalize => getter(&heap, &schema, "anyOf", 81),
                            Stage::Checker => getter(&heap, &schema, "const", 82),
                        }
                        let result = match &staged {
                            Some(staged) => {
                                prepare_staged_graph_arguments(staged, &member, &values, &schema)
                            }
                            None => {
                                prepare_owned_graph_arguments(&owner, &member, &values, &schema)
                            }
                        };
                        assert_eq!(
                            runtime.reads.load(Ordering::SeqCst),
                            1,
                            "{stage:?}/{mutation:?}/{failure:?}"
                        );
                        // Fixed validation.ts:684 reads anyOf during normalization;
                        // json-schema-validator.ts:259-260 reads it again during checking.
                        // A first callback failure stops before that second reached Get.
                        let expected_effects = if matches!(stage, Stage::Normalize)
                            && matches!(failure, Failure::None)
                        {
                            2
                        } else {
                            1
                        };
                        assert_eq!(
                            runtime.effects.load(Ordering::SeqCst),
                            expected_effects,
                            "{stage:?}/{mutation:?}/{failure:?}"
                        );
                        if matches!(mutation, Mutation::None | Mutation::Cache)
                            && matches!(failure, Failure::None)
                        {
                            let prepared = result.unwrap();
                            assert!(
                                prepared.raw_arguments.same_identity(&raw)
                                    && prepared.normalized_arguments.same_identity(&raw)
                            );
                            assert!(matches!(
                                prepared.validation,
                                GraphContextValidation::Accept(_)
                            ));
                        } else {
                            let error = result.unwrap_err();
                            if !matches!(mutation, Mutation::None | Mutation::Cache) {
                                assert!(has_owner_failure(&error), "{error:?}");
                            }
                            if matches!(failure, Failure::Native) {
                                assert!(keeps_thrown(&error, &thrown), "{error:?}");
                            }
                            if !matches!(mutation, Mutation::None | Mutation::Cache)
                                && !matches!(failure, Failure::None)
                            {
                                assert!(
                                    matches!(error, ActualProviderError::Compound { .. }),
                                    "{error:?}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}
