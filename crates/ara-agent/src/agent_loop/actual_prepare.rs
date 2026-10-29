//! Full fixed OMP staged prepare, before the existing assistant's final pin.
//! Source: agent-loop.ts:923–932,2234–2362 at
//! 596f2da7101178214aa27a753529d15e6b7ad91d (MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md.
//!
//! Original handles/slots remain canonical. This prepares receipts; it does not
//! grant an effect, execute tools, or substitute a new Agent/Session.

use super::*;
use crate::agent_message::{CurrentArgumentsSlot, StagedGraphPreparation};
use crate::tool::{ActualToolBinding, ActualToolSource, cursor_exec_resolved_key};
use ara_ai::validation::{GraphToolValidationError, validate_graph_tool_arguments};
use ara_rpc::{
    WireString,
    value_heap::{ValueError, ValueHandle, ValueKind, ValueSnapshot},
};

#[derive(Clone, Debug)]
#[allow(dead_code)] // A retains full receipts; effect worker/consumers remain B/C.
pub(super) struct PreparedActualCall {
    pub call: ValueHandle,
    pub call_id: WireString,
    pub slot: CurrentArgumentsSlot,
    pub tool: Option<ActualToolBinding>,
    pub args: ValueHandle,
    pub validation_error_message: Option<ValueHandle>,
    pub validation_error: Option<Arc<GraphToolValidationError>>,
    pub blocked: bool,
    pub block_reason: Option<ValueHandle>,
    pub prepare_error: Option<Arc<ActualProviderError>>,
}

#[derive(Default)]
pub(super) struct PreparedActualBatch {
    // Keep each original member receipt; lookup by native ID is last-write-wins
    // like Map.set. B must use the dispatch member's own Owned slot separately.
    pub calls: Vec<PreparedActualCall>,
}
impl PreparedActualBatch {
    #[allow(dead_code)] // Native last-write-wins lookup is consumed by B.
    pub fn get(&self, id: &WireString) -> Option<&PreparedActualCall> {
        self.calls.iter().rev().find(|call| &call.call_id == id)
    }
    pub fn bind_owned(&mut self, slots: &[CurrentArgumentsSlot]) -> Result<(), ActualProviderError> {
        for call in &mut self.calls {
            let fresh = slots
                .iter()
                .find(|slot| slot.matches_member(&call.call))
                .ok_or(ActualProviderError::Protocol("prepared call is absent from finalized Owned slots"))?;
            call.slot.require_owned_rebinding(fresh)?;
            call.slot = fresh.clone();
        }
        Ok(())
    }
}

fn both<T>(
    result: Result<T, ActualProviderError>,
    admission: Result<(), ActualProviderError>,
) -> Result<T, ActualProviderError> {
    match (result, admission) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(primary), Err(secondary)) => {
            Err(ActualProviderError::Compound { primary: Arc::new(primary), secondary: Arc::new(secondary) })
        }
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
    }
}
fn guarded<T>(
    slot: &CurrentArgumentsSlot,
    action: impl FnOnce() -> Result<T, ActualProviderError>,
) -> Result<T, ActualProviderError> {
    slot.recheck_current()?;
    let result = action();
    both(result, slot.recheck_current().map_err(Into::into))
}
fn staged_guarded<T>(
    staged: &StagedGraphPreparation,
    action: impl FnOnce() -> Result<T, ActualProviderError>,
) -> Result<T, ActualProviderError> {
    staged.recheck_current()?;
    let result = action();
    both(result, staged.recheck_current().map_err(Into::into))
}
fn native(error: &ActualProviderError) -> bool {
    match error {
        ActualProviderError::Value(error) => error.is_native_exception(),
        ActualProviderError::Owner(error) => error.is_native_exception(),
        // Rust callback panics retain an explicit callback receipt, not a fake
        // Graph Error. Like native hook throws, they reach the scheduled slot.
        ActualProviderError::Callback(_) => true,
        _ => false,
    }
}
fn get(values: &SessionValues, object: &ValueHandle, key: &str) -> Result<ValueHandle, ActualProviderError> {
    Ok(values.heap().get(object, &key.into(), values.runtime())?)
}
fn nullish(values: &SessionValues, value: &ValueHandle) -> Result<bool, ActualProviderError> {
    Ok(matches!(values.heap().kind(value)?, ValueKind::Null | ValueKind::Undefined))
}
fn own(values: &SessionValues, object: &ValueHandle, key: &str, value: ValueHandle) -> Result<(), ActualProviderError> {
    Ok(values.heap().define_own_data(object, key.into(), value, values.runtime())?)
}
fn text(values: &SessionValues, value: &ValueHandle) -> Result<WireString, ActualProviderError> {
    match values.heap().snapshot(value)? {
        ValueSnapshot::String(value) => Ok(value),
        _ => Err(ActualProviderError::Protocol("prepared native call ID must be admitted Text")),
    }
}
fn method(
    values: &SessionValues,
    object: &ValueHandle,
    key: &str,
    args: &[ValueHandle],
) -> Result<ValueHandle, ActualProviderError> {
    let function = get(values, object, key)?;
    Ok(values.heap().call(&function, object, args, values.runtime())?)
}

fn extract_intent(
    values: &SessionValues,
    args: &ValueHandle,
) -> Result<(Option<ValueHandle>, ValueHandle), ActualProviderError> {
    // Destructuring Gets i before CopyDataProperties excludes it (no second i
    // getter). Null/undefined destructuring still throws at the original Get.
    let intent = get(values, args, "i")?;
    let stripped = values.heap().spread_excluding(args, &["i".into()], values.runtime())?;
    if values.heap().kind(&intent)? != ValueKind::String {
        return Ok((None, stripped));
    }
    let trimmed = method(values, &intent, "trim", &[])?;
    // The source creates a new regexp literal on every extractIntent call.
    let pattern = values.heap().regexp(r"\s*\.+$".into(), "")?;
    let empty = values.heap().string("".into())?;
    let trimmed = method(values, &trimmed, "replace", &[pattern, empty])?;
    let trimmed = method(values, &trimmed, "trim", &[])?;
    let length = get(values, &trimmed, "length")?;
    let present = values.heap().to_number(&length, values.runtime())? > 0.0;
    Ok((present.then_some(trimmed), stripped))
}

fn annotate_intent(call: &PreparedActualCall, values: &SessionValues, owns_intent: bool,
    execution_args: &ValueHandle) -> Result<ValueHandle, ActualProviderError> {
    let (intent, stripped) = if owns_intent {
        (None, execution_args.clone())
    } else {
        let raw = call.slot.read(values.runtime())?; // Native's third arguments Get is skipped for schema-owned i.
        guarded(&call.slot, || extract_intent(values, &raw))?
    };
    if let Some(intent) = intent {
        guarded(&call.slot, || Ok(values.heap().assign(&call.call, "intent".into(), intent, values.runtime())?))?;
        return Ok(stripped);
    }
    if let Some(tool) = &call.tool {
        if nullish(values, &tool.source)? {
            return Ok(stripped);
        }
        let policy = guarded(&call.slot, || get(values, &tool.source, "intent"))?;
        if values.heap().kind(&policy)? == ValueKind::Function {
            // typeof tool.intent precedes the second Get in tool.intent(args).
            let derived = guarded(&call.slot, || {
                let value = method(values, &tool.source, "intent", std::slice::from_ref(&stripped))?;
                if nullish(values, &value)? {
                    Ok(None)
                } else {
                    let value = method(values, &value, "trim", &[])?;
                    if values.heap().truthy(&value)? {
                        values.heap().assign(&call.call, "intent".into(), value, values.runtime())?;
                    }
                    Ok(Some(()))
                }
            });
            if let Err(error) = derived {
                // Native intent function failure is nonfatal, but unsupported
                // Host/admission is never claimed to be a native throw.
                if !native(&error) {
                    return Err(error);
                }
            }
        }
    }
    Ok(stripped)
}

fn failure_message(
    error: &GraphToolValidationError,
    config: &AdmittedExecution,
    values: &SessionValues,
) -> Result<ValueHandle, ActualProviderError> {
    match error {
        GraphToolValidationError::Validation(message) | GraphToolValidationError::ToolNotFound { message, .. } => {
            Ok(values.heap().string(message.clone())?)
        }
        GraphToolValidationError::Runtime(error @ ValueError::Thrown(_)) => {
            config.hooks.tool_error_message_actual(error, values)
        }
        GraphToolValidationError::Runtime(
            ValueError::TypeError(message)
            | ValueError::SyntaxError(message)
            | ValueError::RangeError(message)
            | ValueError::DataCloneError(message),
        ) => Ok(values.heap().string(message.as_str().into())?),
        GraphToolValidationError::Runtime(error) => Err(error.clone().into()),
    }
}

fn validate(
    call: &mut PreparedActualCall,
    args: &ValueHandle,
    config: &AdmittedExecution,
    values: &SessionValues,
) -> Result<Option<ValueHandle>, ActualProviderError> {
    let validation = guarded(&call.slot, || {
        Ok((|| {
            match &call.tool {
                Some(tool) if values.heap().truthy(&tool.source)? => {
                    // Native {...toolCall, arguments:args}; original spread Gets are
                    // observable, then arguments is replaced only on this facade.
                    let facade = values.heap().spread(&call.call, values.runtime())?;
                    values.heap().define_own_data(&facade, "arguments".into(), args.clone(), values.runtime())?;
                    validate_graph_tool_arguments(values.factory(), &tool.source, &facade)
                }
                _ => {
                    let name = values.heap().get(&call.call, &"name".into(), values.runtime())?;
                    let name = values.heap().to_string(&name, values.runtime())?;
                    let mut units = WireString::from("Tool ").units().to_vec();
                    units.extend(name.units());
                    units.extend(WireString::from(" not found").units());
                    Err(GraphToolValidationError::Validation(WireString::from_units(units)))
                }
            }
        })())
    })?;
    let error = match validation {
        Ok(value) => {
            values.check_handle(&value)?;
            return Ok(Some(value));
        }
        Err(error) => error,
    };
    if let GraphToolValidationError::Runtime(error) = &error {
        if !error.is_native_exception() {
            return Err(error.clone().into());
        }
    }
    let lenient = if let Some(tool) = &call.tool {
        if nullish(values, &tool.source)? {
            false
        } else {
            guarded(&call.slot, || {
                let value = get(values, &tool.source, "lenientArgValidation")?;
                Ok(values.heap().truthy(&value)?)
            })?
        }
    } else {
        false
    };
    if lenient {
        return guarded(&call.slot, || {
            let fallback = values.heap().spread(args, values.runtime())?;
            for key in ["__parseError", "__rawJson"] {
                if !values.heap().delete_own(&fallback, &key.into(), values.runtime())? {
                    return Err(
                        ValueError::TypeError("Cannot delete property from lenient argument copy".into()).into()
                    );
                }
            }
            Ok(Some(fallback))
        });
    }
    call.args = guarded(&call.slot, || {
        if values.heap().has(args, &"__parseError".into(), values.runtime())? {
            let only_error = values.heap().object()?;
            own(values, &only_error, "__parseError", get(values, args, "__parseError")?)?;
            Ok(only_error)
        } else {
            Ok(args.clone())
        }
    })?;
    let message = guarded(&call.slot, || failure_message(&error, config, values))?;
    values.check_handle(&message)?;
    call.validation_error_message = Some(message);
    call.validation_error = Some(Arc::new(error));
    Ok(None)
}

fn context<'a>(
    owner: &'a AgentMessage,
    call: &'a PreparedActualCall,
    tool: &'a ActualToolBinding,
    source: Option<&'a dyn ActualToolSource>,
    messages: &'a [AgentMessage],
    config: &'a AdmittedExecution,
    values: &'a SessionValues,
) -> ActualBeforeToolCallContext<'a> {
    ActualBeforeToolCallContext {
        assistant_message: owner,
        tool_call: &call.call,
        tool,
        args: &call.args,
        context: ActualAgentContextView { system_prompt: &config.system_prompt, messages, tool_source: source },
        values,
    }
}

#[allow(clippy::too_many_arguments)]
async fn prepare_call(
    owner: &AgentMessage,
    staged: &StagedGraphPreparation,
    member: ValueHandle,
    source: Option<&dyn ActualToolSource>,
    messages: &[AgentMessage],
    config: &AdmittedExecution,
    ctl: &RunControl,
    values: &SessionValues,
) -> Result<PreparedActualCall, ActualProviderError> {
    let slot = staged.current_arguments_slot(&member)?;
    let tool = guarded(&slot, || match source {
        Some(source) => super::actual::resolve_actual_tool_for_call(source, &member, values),
        None => Ok(None),
    })?;
    let args = slot.read(values.runtime())?; // entry args at source 2294.
    let call_id = guarded(&slot, || text(values, &get(values, &member, "id")?))?; // Map.set at 2295.
    let mut call = PreparedActualCall {
        call: member,
        call_id,
        slot,
        tool,
        args,
        validation_error_message: None,
        validation_error: None,
        blocked: false,
        block_reason: None,
        prepare_error: None,
    };
    let execution_args = call.slot.read(values.runtime())?; // independent Get at 2296.
    let execution_args = if config.tracing() {
        let owns_intent = guarded(&call.slot, || {
            match &call.tool {
                Some(tool) if !nullish(values, &tool.source)? => {
                    let schema=ara_ai::graph_tool_wire_schema_in(values.heap(),values.runtime(),&tool.source)?;
                    Ok(ara_ai::schema_validator::graph_schema_defines_property_in(
                        values.heap(),values.runtime(),&schema,"i")?)
                },
                _ => Ok(false),
            }
        })?;
        annotate_intent(&call, values, owns_intent, &execution_args)?
    } else { execution_args };
    let Some(effective) = validate(&mut call, &execution_args, config, values)? else {
        return Ok(call);
    };
    call.args = effective;
    let Some(tool) = call.tool.clone() else {
        return Ok(call);
    };
    if !values.heap().truthy(&tool.source)? {
        return Ok(call);
    }
    let callback = std::panic::AssertUnwindSafe(
        config
            .hooks
            .before_tool_dispatch_actual(context(owner, &call, &tool, source, messages, config, values), &ctl.token),
    )
    .catch_unwind()
    .await;
    let callback = match callback {
        Ok(result) => result,
        Err(panic) => {
            Err(ActualProviderError::Callback(format!("Actual before tool hook panicked: {}", panic_text(panic))))
        }
    };
    let before = match both(callback, call.slot.recheck_current().map_err(Into::into)) {
        Ok(result) => result,
        Err(error) if native(&error) => {
            call.prepare_error = Some(Arc::new(error));
            return Ok(call);
        }
        Err(error) => return Err(error),
    };
    if let Some(before) = before {
        values.check_handle(&before)?;
        if !nullish(values, &before)? {
            let blocked = guarded(&call.slot, || Ok(values.heap().truthy(&get(values, &before, "block")?)?))?;
            if blocked {
                call.blocked = true;
                call.block_reason = Some(guarded(&call.slot, || get(values, &before, "reason"))?);
                return Ok(call);
            }
            // Three distinct beforeResult.args Gets: condition, validation,
            // original setter value. Never substitute the normalized revision.
            let first = guarded(&call.slot, || get(values, &before, "args"))?;
            if values.heap().kind(&first)? != ValueKind::Undefined {
                let revision = guarded(&call.slot, || get(values, &before, "args"))?;
                let Some(revised) = validate(&mut call, &revision, config, values)? else {
                    return Ok(call);
                };
                let assignment = guarded(&call.slot, || get(values, &before, "args"))?;
                call.slot.assign(&assignment, values.runtime())?;
                call.args = revised;
            }
        }
    }
    let permission = std::panic::AssertUnwindSafe(
        config
            .hooks
            .before_tool_call_actual(context(owner, &call, &tool, source, messages, config, values), &ctl.token),
    )
    .catch_unwind()
    .await;
    let permission = match permission {
        Ok(result) => result,
        Err(panic) => {
            Err(ActualProviderError::Callback(format!("Actual tool permission panicked: {}", panic_text(panic))))
        }
    };
    match both(permission, call.slot.recheck_current().map_err(Into::into)) {
        Ok(ActualToolDecision::Allow) => {}
        Ok(ActualToolDecision::Block(reason)) => {
            call.blocked = true;
            call.block_reason = reason.map(|reason| values.heap().string(reason)).transpose()?;
        }
        Err(error) if native(&error) => call.prepare_error = Some(Arc::new(error)),
        Err(error) => return Err(error),
    }
    Ok(call)
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn prepare_staged_actual_calls(
    owner: &AgentMessage,
    staged: &StagedGraphPreparation,
    source: Option<&dyn ActualToolSource>,
    messages: &[AgentMessage],
    config: &AdmittedExecution,
    ctl: &RunControl,
    values: &SessionValues,
) -> Result<PreparedActualBatch, ActualProviderError> {
    let root = match owner.body_snapshot() {
        AgentMessageBody::Graph(root) => root,
        _ => return Err(ActualProviderError::Protocol("actual prepare requires its original Graph owner")),
    };
    let content = staged_guarded(staged, || get(values, &root, "content"))?;
    let mut iterator = staged_guarded(staged, || Ok(values.heap().get_iterator(&content, values.runtime())?))?;
    let mut batch = PreparedActualBatch::default();
    loop {
        let next = staged_guarded(staged, || Ok(values.heap().iterator_next(&mut iterator, values.runtime())?))?;
        let Some(member) = next else {
            break;
        };
        let body = async {
            let is_call = staged_guarded(staged, || {
                Ok(matches!(values.heap().snapshot(&get(values, &member, "type")?)?,
                    ValueSnapshot::String(kind) if kind.equals_ascii("toolCall")))
            })?;
            if !is_call {
                return Ok(None);
            }
            let resolved = staged_guarded(staged, || {
                let marker = values.heap().get(&member, &cursor_exec_resolved_key(values.heap())?, values.runtime())?;
                Ok(matches!(values.heap().snapshot(&marker)?, ValueSnapshot::Boolean(true)))
            })?;
            if resolved {
                return Ok(None);
            } // No arguments Get for a resolved call.
            if !staged_guarded(staged, || Ok(config.hooks.supports_actual_tool_hooks()))? {
                return Err(ActualProviderError::Unsupported("full actual tool preparation hooks"));
            }
            prepare_call(owner, staged, member, source, messages, config, ctl, values).await.map(Some)
        }
        .await;
        match body {
            Ok(Some(call)) => batch.calls.push(call),
            Ok(None) => {}
            Err(primary) => {
                let close =
                    staged_guarded(staged, || Ok(values.heap().iterator_close(&mut iterator, values.runtime())?));
                return Err(match close {
                    Ok(()) => primary,
                    Err(close) if native(&primary) && native(&close) => primary,
                    Err(secondary) => {
                        ActualProviderError::Compound { primary: Arc::new(primary), secondary: Arc::new(secondary) }
                    }
                });
            }
        }
    }
    Ok(batch)
}

/// Extend the existing owned-consumer family; no new per-slot test entrance.
#[cfg(test)]
pub(super) fn observe_full_staged_prepare_group() {
    use ara_rpc::value_heap::{
        CallableTarget, CapabilityId, NoRuntime, PropertyDescriptor, RuntimeReply, RuntimeRequest, ValueHeap,
        ValueRuntime,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn own(heap: &Arc<ValueHeap>, target: &ValueHandle, key: &str, value: ValueHandle) {
        heap.define_own_data(target, key.into(), value, &NoRuntime).unwrap();
    }
    struct Runtime {
        heap: Arc<ValueHeap>,
        raw: ValueHandle,
        validation_revision: ValueHandle,
        published_revision: ValueHandle,
        assigned: Mutex<Option<ValueHandle>>,
        argument_reads: AtomicUsize,
        revision_reads: AtomicUsize,
        writes: AtomicUsize,
    }
    impl ValueRuntime for Runtime {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            let value = match request {
                RuntimeRequest::Call { capability: CapabilityId(1101), .. } => {
                    self.argument_reads.fetch_add(1, Ordering::SeqCst);
                    self.assigned.lock().unwrap().clone().unwrap_or_else(|| self.raw.clone())
                }
                RuntimeRequest::Call { capability: CapabilityId(1102), arguments, .. } => {
                    self.writes.fetch_add(1, Ordering::SeqCst);
                    *self.assigned.lock().unwrap() = Some(arguments[0].clone());
                    self.heap.undefined()?
                }
                RuntimeRequest::Call { capability: CapabilityId(1103), .. } => {
                    let index = self.revision_reads.fetch_add(1, Ordering::SeqCst);
                    if index < 2 { self.validation_revision.clone() } else { self.published_revision.clone() }
                }
                RuntimeRequest::Call { capability: CapabilityId(1104), arguments, .. } => {
                    let intent=self.heap.get(&arguments[0],&"i".into(),self)?;
                    assert!(matches!(self.heap.snapshot(&intent)?,ValueSnapshot::String(text) if text.equals_ascii("  intent...  ")));
                    self.heap.string("derived own value".into())?
                }
                request => return Err(ValueError::MissingCapability(request.capability())),
            };
            Ok(RuntimeReply::Value(value))
        }
    }
    struct Tool;
    #[async_trait]
    impl AgentTool for Tool {
        fn definition(&self) -> ara_ai::Tool {
            panic!("full actual prepare called typed definition")
        }
        async fn execute(
            &self,
            _: &str,
            _: JsonObject,
            _: CancellationToken,
            _: crate::tool::UpdateFn,
        ) -> Result<ToolOutput, crate::tool::ToolError> {
            panic!("A must not execute any tool")
        }
    }
    struct Source {
        collection: ValueHandle,
        tool: ValueHandle,
        capability: Arc<dyn AgentTool>,
        reads: AtomicUsize,
    }
    impl ActualToolSource for Source {
        fn current_tools(&self, _: &SessionValues) -> Result<Option<ValueHandle>, ActualProviderError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            Ok(Some(self.collection.clone()))
        }
        fn bind_selected(
            &self,
            selected: &ValueHandle,
            values: &SessionValues,
        ) -> Result<ActualToolBinding, ActualProviderError> {
            let binding = ActualToolBinding { source: self.tool.clone(), capability: self.capability.clone() };
            binding.check_selected(selected, values)?;
            Ok(binding)
        }
    }
    struct Provider;
    impl ModelProvider for Provider {
        fn stream(&self, _: &Model, _: &Context, _: CallOptions) -> ara_ai::AssistantStream {
            panic!("A source fixture is not a model trial")
        }
    }
    struct Hooks {
        mode: &'static str,
        before: ValueHandle,
        thrown: ValueHandle,
        permission: AtomicUsize,
    }
    #[async_trait]
    impl LoopHooks for Hooks {
        fn supports_actual_tool_hooks(&self) -> bool {
            true
        }
        async fn transform_assistant_graph(
            &self,
            _: &AgentMessage,
            _: &CancellationToken,
            _: &SessionValues,
        ) -> Result<(), ActualProviderError> {
            Ok(())
        }
        async fn before_tool_dispatch_actual(
            &self,
            context: ActualBeforeToolCallContext<'_>,
            _: &CancellationToken,
        ) -> Result<Option<ValueHandle>, ActualProviderError> {
            assert!(context.context.tool_source.is_some());
            match self.mode {
                "throw" => Err(ValueError::Thrown(self.thrown.clone()).into()),
                "owner-mutation" => {
                    context.values.heap().assign(
                        context.tool_call,
                        "name".into(),
                        context.values.heap().string("changed".into())?,
                        context.values.runtime(),
                    )?;
                    Err(ValueError::Thrown(self.thrown.clone()).into())
                }
                "lenient" | "owned-i" | "derived-owned-i" => Ok(None),
                _ => Ok(Some(self.before.clone())),
            }
        }
        async fn before_tool_call_actual(
            &self,
            context: ActualBeforeToolCallContext<'_>,
            _: &CancellationToken,
        ) -> Result<ActualToolDecision, ActualProviderError> {
            self.permission.fetch_add(1, Ordering::SeqCst);
            if matches!(self.mode, "revision" | "intent-revision") {
                let text = context.values.heap().get(context.args, &"text".into(), context.values.runtime())?;
                assert!(
                    matches!(context.values.heap().snapshot(&text)?, ValueSnapshot::String(text) if text.equals_ascii("validated"))
                );
                assert!(!context.values.heap().has_own(context.args, &"optional".into(), context.values.runtime())?);
            }
            Ok(ActualToolDecision::Allow)
        }
    }

    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        for mode in ["revision", "intent-revision", "validation", "lenient", "block", "throw", "owner-mutation", "cursor", "owned-i", "derived-owned-i"] {
            let heap = ValueHeap::new().unwrap();
            let raw = heap.import_serde(&if matches!(mode, "validation" | "lenient") {
                json!({"__parseError":"broken", "__rawJson":"{private-fixture", "other":1})
            } else if matches!(mode,"owned-i"|"derived-owned-i") {
                json!({"text":"original", "optional":"present", "i":"  intent...  "})
            } else { json!({"text":"original", "optional":null, "i":"  intent...  "}) }).unwrap();
            let validation_revision = heap.import_serde(&json!({"text":"validated","optional":null})).unwrap();
            let published_revision = heap.import_serde(&json!({"text":"published","optional":null})).unwrap();
            let runtime = Arc::new(Runtime { heap: heap.clone(), raw: raw.clone(),
                validation_revision: validation_revision.clone(), published_revision: published_revision.clone(),
                assigned: Mutex::new(None), argument_reads: AtomicUsize::new(0),
                revision_reads: AtomicUsize::new(0), writes: AtomicUsize::new(0) });
            let values = Arc::new(SessionValues::new(heap.clone(), runtime.clone()).unwrap());
            let call = heap.object().unwrap();
            for (key, value) in [("type","toolCall"),("id","call"),("name","echo")] {
                own(&heap, &call, key, heap.string(value.into()).unwrap());
            }
            heap.define_own(&call, "arguments".into(), PropertyDescriptor::Accessor {
                get: Some(CallableTarget::Host(CapabilityId(1101))), set: Some(CallableTarget::Host(CapabilityId(1102))),
                enumerable: true, configurable: true,
            }, &NoRuntime).unwrap();
            if mode == "cursor" {
                heap.assign(&call, cursor_exec_resolved_key(&heap).unwrap(), heap.boolean(true).unwrap(), &NoRuntime).unwrap();
            }
            let graph = heap.object().unwrap();
            own(&heap, &graph, "role", heap.string("assistant".into()).unwrap());
            own(&heap, &graph, "stopReason", heap.string("toolUse".into()).unwrap());
            own(&heap, &graph, "content", heap.array(vec![call.clone()]).unwrap());
            let source_tool = heap.import_serde(&json!({"name":"echo", "parameters":{
                "type":"object","properties":{"text":{"type":"string"},"optional":{"type":"string"}},"required":["text"]
            },"lenientArgValidation":mode=="lenient"})).unwrap();
            if matches!(mode,"owned-i"|"derived-owned-i") {
                own(&heap,&source_tool,"parameters",heap.import_serde(&json!({"$ref":"#/$defs/args","$defs":{"args":{
                    "type":"object","properties":{"i":{"type":"string"},"text":{"type":"string"},"optional":{"type":"string"}},
                    "required":["i","text"]}}})).unwrap());
                if mode=="derived-owned-i" {own(&heap,&source_tool,"intent",heap.function(CapabilityId(1104)).unwrap());}
                // b91 optional-null normalization does not descend through a
                // root-only $ref. Preserve the failed fixture as a rejection
                // control; only the new happy modes use a valid optional value.
                let invalid_call=heap.import_serde(&json!({"name":"echo","arguments":{
                    "text":"original","optional":null,"i":"  intent...  "}})).unwrap();
                match validate_graph_tool_arguments(values.factory(),&source_tool,&invalid_call) {
                    Err(GraphToolValidationError::Validation(message)) => {
                        let message=message.to_utf8().unwrap();
                        assert!(message.contains("optional")&&message.contains("string"),"{mode}: {message}");
                        eprintln!("APPEND_UPGRADE_ROOT_REF_NULL_REJECT {mode}: {message}");
                    },
                    other=>panic!("{mode}: root-ref/null must retain its source validation rejection: {other:?}"),
                }
            }
            let source = Arc::new(Source { collection: heap.array(vec![source_tool.clone()]).unwrap(),
                tool: source_tool, capability: Arc::new(Tool), reads: AtomicUsize::new(0) });
            let before = heap.object().unwrap();
            if mode == "block" {
                own(&heap, &before, "block", heap.boolean(true).unwrap());
                own(&heap, &before, "reason", heap.symbol(Some("raw reason".into())).unwrap());
            } else {
                heap.define_own(&before, "args".into(), PropertyDescriptor::Accessor {
                    get: Some(CallableTarget::Host(CapabilityId(1103))), set: None, enumerable: true, configurable: true,
                }, &NoRuntime).unwrap();
            }
            let thrown = heap.symbol(Some("original hook throw".into())).unwrap();
            let hooks = Arc::new(Hooks { mode, before, thrown: thrown.clone(), permission: AtomicUsize::new(0) });
            let config = AdmittedExecution {
                config: AgentConfig { model: Model { id:"prepare".into(),api:"openai-completions".into(),
                    provider:"fixture".into(),base_url:String::new(),reasoning:false,max_tokens:None,
                    context_window:None,tokenizer:None }, provider:Arc::new(Provider),system_prompt:vec![],
                    tools:vec![],tool_choice:None,max_tokens:None,temperature:None,deadline:None,
                    max_model_calls:Some(1),hooks:hooks.clone(),interrupt_mode:InterruptMode::Immediate },
                provider_context: Some(ProviderContextBinding { build_options: crate::BuildOptions {
                    intent_tracing: matches!(mode,"intent-revision"|"owned-i"|"derived-owned-i"), ..Default::default()
                }, ..Default::default() }),
            };
            let ctl = RunControl::new(&CancellationToken::new(), None);
            let sink = crate::RecordingSink::default();
            let mut messages = Vec::new();
            let mut stream = super::actual::ActualStreamState::new(values.clone(), None, Some(source.clone()), None);
            stream.update(&mut messages, &sink, ara_ai::ActualAssistantEvent::Start { partial: graph.clone() }).await.unwrap();
            let final_assistant = stream.finish(&mut messages, &config, &ctl, &sink, Some(graph.clone()), None, true, false).await;
            assert!(matches!(final_assistant.message.body_snapshot(), AgentMessageBody::Graph(current) if current.same_identity(&graph)));
            assert_eq!(messages.len(), 1);
            assert!(messages[0].same_record(&final_assistant.message));
            assert!(source.capability.actual_invoker().is_none(), "typed tool never opts in implicitly");
            if mode == "owner-mutation" {
                assert!(final_assistant.prepared_dispatch.is_none());
                assert!(sink.events.lock().await.iter().any(|event| matches!(event, AgentEvent::ActualMessageEnd {..})));
                fn contains_thrown(error: &ActualProviderError, thrown: &ValueHandle) -> bool {
                    match error {
                        ActualProviderError::Value(ValueError::Thrown(value)) => value.same_identity(thrown),
                        ActualProviderError::Compound {primary,secondary} => contains_thrown(primary,thrown)||contains_thrown(secondary,thrown),
                        _ => false,
                    }
                }
                assert!(contains_thrown(final_assistant.failure.as_ref().unwrap(), &thrown));
                continue;
            }
            let batch = final_assistant.prepared_dispatch.as_ref().unwrap();
            if mode == "cursor" {
                assert!(batch.calls.is_empty());
                assert_eq!(runtime.argument_reads.load(Ordering::SeqCst), 0);
                assert_eq!(source.reads.load(Ordering::SeqCst), 0);
                assert_eq!(hooks.permission.load(Ordering::SeqCst), 0);
                continue;
            }
            assert!(final_assistant.failure.is_none(), "staged preparation permits the original actual execution path");
            assert_eq!(batch.calls.len(), 1);
            let prepared = batch.get(&WireString::from("call")).unwrap();
            assert!(prepared.call.same_identity(&call));
            prepared.slot.recheck_current().unwrap();
            assert_eq!(source.reads.load(Ordering::SeqCst), 1);
            assert_eq!(runtime.argument_reads.load(Ordering::SeqCst), match mode { "revision"=>4,"intent-revision"=>5,_=>3 });
            match mode {
                "revision" | "intent-revision" => {
                    assert_eq!(runtime.revision_reads.load(Ordering::SeqCst), 3);
                    assert_eq!(runtime.writes.load(Ordering::SeqCst), 1);
                    assert_eq!(hooks.permission.load(Ordering::SeqCst), 1);
                    assert!(runtime.assigned.lock().unwrap().as_ref().unwrap().same_identity(&published_revision));
                    assert!(!prepared.args.same_identity(&published_revision));
                    assert!(!prepared.args.same_identity(&validation_revision));
                    assert!(prepared.slot.read(values.runtime()).unwrap().same_identity(&published_revision));
                    if mode=="intent-revision" {
                        assert!(matches!(heap.snapshot(&heap.get(&call,&"intent".into(),values.runtime()).unwrap()).unwrap(), ValueSnapshot::String(intent) if intent.equals_ascii("intent")));
                    }
                }
                "owned-i" | "derived-owned-i" => {
                    assert!(prepared.validation_error_message.is_none(),"{mode}: {:?}",prepared.validation_error);
                    let optional=heap.get(&prepared.args,&"optional".into(),values.runtime()).unwrap();
                    assert!(matches!(heap.snapshot(&optional).unwrap(),ValueSnapshot::String(text) if text.equals_ascii("present")));
                    let intent=heap.get(&prepared.args,&"i".into(),values.runtime()).unwrap();
                    assert!(matches!(heap.snapshot(&intent).unwrap(),ValueSnapshot::String(text) if text.equals_ascii("  intent...  ")));
                    assert_eq!(hooks.permission.load(Ordering::SeqCst),1);
                    let label=heap.get(&call,&"intent".into(),values.runtime()).unwrap();
                    if mode=="owned-i" {assert_eq!(heap.kind(&label).unwrap(),ValueKind::Undefined);}
                    else {assert!(matches!(heap.snapshot(&label).unwrap(),ValueSnapshot::String(text) if text.equals_ascii("derived own value")));}
                }
                "validation" => {
                    assert!(prepared.validation_error_message.is_some());
                    assert!(heap.has_own(&prepared.args,&"__parseError".into(),values.runtime()).unwrap());
                    assert!(!heap.has_own(&prepared.args,&"__rawJson".into(),values.runtime()).unwrap());
                    assert_eq!(hooks.permission.load(Ordering::SeqCst), 0);
                }
                "lenient" => {
                    assert!(prepared.validation_error_message.is_none());
                    assert!(!heap.has_own(&prepared.args,&"__parseError".into(),values.runtime()).unwrap());
                    assert!(!heap.has_own(&prepared.args,&"__rawJson".into(),values.runtime()).unwrap());
                    assert_eq!(hooks.permission.load(Ordering::SeqCst), 1);
                }
                "block" => {
                    assert!(prepared.blocked && prepared.block_reason.is_some());
                    assert_eq!(runtime.revision_reads.load(Ordering::SeqCst), 0);
                    assert_eq!(hooks.permission.load(Ordering::SeqCst), 0);
                }
                "throw" => assert!(matches!(prepared.prepare_error.as_ref().unwrap().as_ref(),
                    ActualProviderError::Value(ValueError::Thrown(value)) if value.same_identity(&thrown))),
                _ => unreachable!(),
            }
            assert!(sink.events.lock().await.iter().any(|event| matches!(event,
                AgentEvent::MessageEnd {message} if message.same_record(&final_assistant.message))));
        }
        super::actual_execute::observe_actual_execution_group().await;
    });
}
