//! Original-domain append-only context, fixed OMP 596f2da7101178214aa27a753529d15e6b7ad91d.
//! MIT; Copyright OMP contributors. See THIRD_PARTY_NOTICES.md.
//! Manager state is inspected between operations; no Graph Get/call/stringify
//! runs under its shared mutex. Reentrant reset/sync therefore observes the
//! same live cursor/log rather than deadlocking or committing a staged copy.

use super::{BuildOptions, SharedAppendOnlyContextManager, StablePrefixSnapshot, prefix_fingerprint};
use crate::{ActualProviderContext, ActualProviderError, ProviderContextError, SessionValues, tool::AgentTool};
use ara_rpc::{
    WireString,
    value_heap::{DeferredValue, PropertyKey, StringifyMode, ValueError, ValueHandle, ValueKind, ValueSnapshot},
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

pub(super) type SharedActualAppendOnlyState = Arc<Mutex<ActualAppendOnlyState>>;

#[derive(Clone, Debug)]
struct PrefixSnapshot {
    fingerprint: String,
    owner_version: u64,
    original_tools: bool,
    prompt: ValueHandle,
    tools: ValueHandle,
    prompt_observation: Vec<WireString>,
    tool_observation: Arc<[ValueHandle]>,
}

#[derive(Clone, Copy, Debug)]
struct DigestMemo {
    version: DeferredValue,
    digest: u32,
}

#[derive(Debug)]
pub(super) struct ActualAppendOnlyState {
    heap_token: u64,
    prefix: Option<PrefixSnapshot>,
    original_key: Option<OriginalPrefixKey>,
    // None is an unobserved freshly cleared Array, allocated on its next read.
    // Clear never reuses the previous array identity.
    log: Option<ValueHandle>,
    last_sync_count: usize,
    // Reentrant resets can make source `length = stableCount` create holes.
    message_digests: Arc<Mutex<Vec<Option<u32>>>>,
    // Neither key nor version owns the heap. Original WeakMap survives reset.
    digest_memo: HashMap<DeferredValue, DigestMemo>,
}

fn state_for(
    manager: &SharedAppendOnlyContextManager,
    values: &SessionValues,
) -> Result<SharedActualAppendOnlyState, ActualProviderError> {
    let state = {
        let mut manager = manager.lock().unwrap_or_else(|poison| poison.into_inner());
        manager
            .actual
            .get_or_insert_with(|| {
                Arc::new(Mutex::new(ActualAppendOnlyState {
                    heap_token: values.heap().heap_token(),
                    prefix: None,
                    original_key: None,
                    log: None,
                    last_sync_count: 0,
                    message_digests: Arc::new(Mutex::new(Vec::new())),
                    digest_memo: HashMap::new(),
                }))
            })
            .clone()
    };
    if state.lock().unwrap_or_else(|poison| poison.into_inner()).heap_token != values.heap().heap_token() {
        return Err(ValueError::ForeignHeap.into());
    }
    Ok(state)
}

pub(super) fn reset_sync_cursor(state: &SharedActualAppendOnlyState) {
    let mut state = state.lock().unwrap_or_else(|poison| poison.into_inner());
    state.log = None;
    state.last_sync_count = 0;
    state.message_digests = Arc::new(Mutex::new(Vec::new()));
    // Deliberately retain digest_memo, exactly as the original WeakMap.
}

pub(super) fn invalidate_prefix(state: &SharedActualAppendOnlyState) {
    let mut state = state.lock().unwrap_or_else(|poison| poison.into_inner());
    state.prefix = None;
    state.original_key = None;
}

fn entries(state: &SharedActualAppendOnlyState, values: &SessionValues) -> Result<ValueHandle, ActualProviderError> {
    let existing = state.lock().unwrap_or_else(|poison| poison.into_inner()).log.clone();
    if let Some(existing) = existing {
        return Ok(existing);
    }
    // Array allocation invokes no runtime, but still occurs outside the lock.
    let fresh = values.heap().array(Vec::new())?;
    let mut state = state.lock().unwrap_or_else(|poison| poison.into_inner());
    Ok(state.log.get_or_insert(fresh).clone())
}

/// Original log entries Array. A clear replaces this identity; slices retain members.
pub fn log_entries_actual(
    manager: &SharedAppendOnlyContextManager,
    values: &SessionValues,
) -> Result<ValueHandle, ActualProviderError> {
    entries(&state_for(manager, values)?, values)
}

pub fn sync_cursor_actual(
    manager: &SharedAppendOnlyContextManager,
    values: &SessionValues,
) -> Result<usize, ActualProviderError> {
    let state = state_for(manager, values)?;
    let count = state.lock().unwrap_or_else(|poison| poison.into_inner()).last_sync_count;
    Ok(count)
}

/// Original public log.clear(): replace entries without resetting the manager
/// cursor/digests. The next sync bounds its stable prefix by this empty log.
pub fn clear_log_actual(
    manager: &SharedAppendOnlyContextManager,
    values: &SessionValues,
) -> Result<(), ActualProviderError> {
    let state = state_for(manager, values)?;
    state.lock().unwrap_or_else(|poison| poison.into_inner()).log = None;
    Ok(())
}

pub fn truncate_log_actual(
    manager: &SharedAppendOnlyContextManager,
    count: isize,
    values: &SessionValues,
) -> Result<(), ActualProviderError> {
    let log = entries(&state_for(manager, values)?, values)?;
    let count = count.max(0) as usize;
    let length = values.heap().array_length(&log, values.runtime())? as usize;
    if count < length {
        values.heap().array_set_length(&log, count as u32, values.runtime())?;
    }
    Ok(())
}

fn push(
    state: &SharedActualAppendOnlyState,
    message: &ValueHandle,
    values: &SessionValues,
) -> Result<(), ActualProviderError> {
    let log = entries(state, values)?;
    let function = values.heap().get(&log, &"push".into(), values.runtime())?;
    values.heap().call(&function, &log, &[message.clone()], values.runtime())?;
    Ok(())
}

pub fn append_message_actual(
    manager: &SharedAppendOnlyContextManager,
    message: &ValueHandle,
    values: &SessionValues,
) -> Result<(), ActualProviderError> {
    values.check_handle(message)?;
    push(&state_for(manager, values)?, message, values)
}

pub fn replace_tail_message_actual(
    manager: &SharedAppendOnlyContextManager,
    message: &ValueHandle,
    values: &SessionValues,
) -> Result<(), ActualProviderError> {
    values.check_handle(message)?;
    let log = entries(&state_for(manager, values)?, values)?;
    let length = values.heap().array_length(&log, values.runtime())?;
    if length > 0 {
        values.heap().assign(&log, (length - 1).to_string().as_str().into(), message.clone(), values.runtime())?;
    }
    Ok(())
}

fn estimate_key(values: &SessionValues) -> Result<PropertyKey, ValueError> {
    values.heap().module_symbol("ara-agent.compaction.message-cache", "omp.messageEstimateVersion")
}

fn nullish(value: &ValueHandle, values: &SessionValues) -> Result<bool, ValueError> {
    Ok(matches!(values.heap().snapshot(value)?, ValueSnapshot::Undefined | ValueSnapshot::Null))
}

/// The private symbol Get precedes every digest memo lookup, including memo hits.
pub fn message_estimate_version_actual(
    message: &ValueHandle,
    values: &SessionValues,
) -> Result<ValueHandle, ActualProviderError> {
    values.check_handle(message)?;
    let version = values.heap().get(message, &estimate_key(values)?, values.runtime())?;
    if nullish(&version, values)? { Ok(values.heap().number(0.0)?) } else { Ok(version) }
}

/// Daily Host adapter, not the complete original invalidator/tokenizer timing.
/// The converter calls this only when an aliased root's owner generation changed.
pub fn publish_owner_estimate_version_actual(
    message: &ValueHandle,
    generation: u64,
    values: &SessionValues,
) -> Result<(), ActualProviderError> {
    values.check_handle(message)?;
    let signed = generation as u32 as i32;
    let version = values.heap().number(f64::from(signed))?;
    values.heap().assign(message, estimate_key(values)?, version, values.runtime())?;
    Ok(())
}

fn strict_same(left: &ValueHandle, right: &ValueHandle, values: &SessionValues) -> Result<bool, ValueError> {
    Ok(match (values.heap().snapshot(left)?, values.heap().snapshot(right)?) {
        (ValueSnapshot::Undefined, ValueSnapshot::Undefined) | (ValueSnapshot::Null, ValueSnapshot::Null) => true,
        (ValueSnapshot::Boolean(left), ValueSnapshot::Boolean(right)) => left == right,
        (ValueSnapshot::Number(left), ValueSnapshot::Number(right)) => left == right,
        (ValueSnapshot::String(left), ValueSnapshot::String(right)) => left == right,
        (ValueSnapshot::BigInt(left), ValueSnapshot::BigInt(right)) => left == right,
        _ => left.same_identity(right),
    })
}

fn field(
    message: &ValueHandle,
    name: &str,
    fallback: Option<&str>,
    values: &SessionValues,
) -> Result<ValueHandle, ActualProviderError> {
    let mut value = values.heap().get(message, &name.into(), values.runtime())?;
    if nullish(&value, values)? {
        if let Some(fallback) = fallback {
            value = values.heap().get(message, &fallback.into(), values.runtime())?;
        }
        if nullish(&value, values)? {
            value = values.heap().null()?;
        }
    }
    Ok(value)
}

fn digest(
    state: &SharedActualAppendOnlyState,
    message: &ValueHandle,
    values: &SessionValues,
) -> Result<u32, ActualProviderError> {
    if !matches!(values.heap().kind(message)?, ValueKind::Object | ValueKind::Array) {
        return Ok(0);
    }
    let version = message_estimate_version_actual(message, values)?;
    let key = values.heap().capture_deferred(message)?;
    let cached = state.lock().unwrap_or_else(|poison| poison.into_inner()).digest_memo.get(&key).copied();
    if let Some(cached) = cached {
        let cached_version = values.heap().restore_deferred(&cached.version)?;
        if strict_same(&cached_version, &version, values)? {
            return Ok(cached.digest);
        }
    }
    let payload = values.heap().object()?;
    for (target, source, fallback) in [
        ("r", "role", None),
        ("c", "content", None),
        ("pp", "providerPayload", None),
        ("tc", "toolCalls", Some("tool_calls")),
        ("tcid", "toolCallId", Some("tool_call_id")),
        ("tn", "toolName", Some("name")),
        ("err", "isError", None),
        ("id", "id", None),
    ] {
        let value = field(message, source, fallback, values)?;
        values.heap().define_own_data(&payload, target.into(), value, values.runtime())?;
    }
    let payload = values
        .heap()
        .stringify(&payload, StringifyMode::Plain, None, values.runtime())?
        .ok_or_else(|| ValueError::TypeError("Cannot read properties of undefined (reading 'length')".into()))?;
    let digest = payload.units().iter().fold(0_u32, |hash, unit| hash.wrapping_mul(31).wrapping_add(u32::from(*unit)));
    let version = values.heap().capture_deferred(&version)?;
    let mut state = state.lock().unwrap_or_else(|poison| poison.into_inner());
    state.digest_memo.try_reserve(1).map_err(|_| ValueError::Allocation)?;
    state.digest_memo.insert(key, DigestMemo { version, digest });
    Ok(digest)
}

fn sync(
    state: &SharedActualAppendOnlyState,
    messages: &[ValueHandle],
    values: &SessionValues,
) -> Result<(), ActualProviderError> {
    let last_count = state.lock().unwrap_or_else(|poison| poison.into_inner()).last_sync_count;
    if messages.len() < last_count {
        reset_sync_cursor(state);
    }
    let last_count = state.lock().unwrap_or_else(|poison| poison.into_inner()).last_sync_count;
    if last_count > 0 {
        let bound = last_count.min(messages.len());
        let mut stable = bound;
        for (index, message) in messages.iter().enumerate().take(bound) {
            // Read the live previous digest AFTER callback-capable messageDigest,
            // as in the source comparison; nested reset/sync is observable.
            let current = digest(state, message, values)?;
            let previous = state.lock().unwrap_or_else(|poison| poison.into_inner()).message_digests.clone();
            let prior = previous.lock().unwrap_or_else(|poison| poison.into_inner()).get(index).copied().flatten();
            if prior != Some(current) {
                stable = index;
                break;
            }
        }
        let log = entries(state, values)?;
        let length = values.heap().array_length(&log, values.runtime())? as usize;
        stable = stable.min(length);
        let last_count = state.lock().unwrap_or_else(|poison| poison.into_inner()).last_sync_count;
        if stable < last_count {
            if stable < length {
                values.heap().array_set_length(&log, stable as u32, values.runtime())?;
            }
            let digests = {
                let mut state = state.lock().unwrap_or_else(|poison| poison.into_inner());
                state.last_sync_count = stable;
                state.message_digests.clone()
            };
            digests.lock().unwrap_or_else(|poison| poison.into_inner()).resize(stable, None);
        }
    }
    let start = state.lock().unwrap_or_else(|poison| poison.into_inner()).last_sync_count;
    for message in messages.iter().skip(start) {
        // Original partial-state contract: append BEFORE digest; a thrown Get/
        // stringify keeps the appended member and does not advance final cursor.
        push(state, message, values)?;
        // Source `this.#messageDigests.push(this.#messageDigest(msg))` captures
        // its Array receiver BEFORE evaluating the callback-capable argument.
        // Reset replaces that Array; the pending push still targets its old one.
        let receiver = state.lock().unwrap_or_else(|poison| poison.into_inner()).message_digests.clone();
        let digest = digest(state, message, values)?;
        let mut receiver = receiver.lock().unwrap_or_else(|poison| poison.into_inner());
        receiver.try_reserve(1).map_err(|_| ValueError::Allocation)?;
        receiver.push(Some(digest));
    }
    state.lock().unwrap_or_else(|poison| poison.into_inner()).last_sync_count = messages.len();
    Ok(())
}

// A key belongs to the original source domain; typed values use their separate
// ordered-value compatibility key on StablePrefix. Never cross those domains.
#[derive(Clone, Debug)]
struct OriginalPrefixKey {
    prompt: Vec<String>,
    tools: ValueHandle,
    options: BuildOptions,
}
fn current_prefix(manager:&SharedAppendOnlyContextManager,state: &SharedActualAppendOnlyState) -> Result<PrefixSnapshot, ProviderContextError> {
    let original=manager.lock().unwrap_or_else(|poison|poison.into_inner()).prefix.original_fingerprint.is_some();
    let prefix=state.lock().unwrap_or_else(|poison| poison.into_inner()).prefix.clone()
        .ok_or(ProviderContextError::PrefixNotBuilt)?;
    if !original || !prefix.original_tools {
        return Err(ActualProviderError::Unsupported("reentrant prefix source-domain change").into());
    }
    Ok(prefix)
}
fn original_key_matches(key:&OriginalPrefixKey,prompt:&[String],tools:&ValueHandle,
    options:&BuildOptions,values:&SessionValues)->Result<bool,ActualProviderError>{
    if key.options!=*options || key.prompt!=prompt { return Ok(false); }
    let heap=values.heap();let runtime=values.runtime();
    let old_length=heap.get(&key.tools,&"length".into(),runtime)?;
    let live_length=heap.get(tools,&"length".into(),runtime)?;
    if !strict_same(&old_length,&live_length,values)? { return Ok(false); }
    let mut index=0u64;
    loop {
        // Source re-reads live toolKeys.length on every loop test.
        let length=heap.get(tools,&"length".into(),runtime)?;
        if !((index as f64) < heap.to_number(&length,runtime)?) { break; }
        let property=PropertyKey::from(index.to_string().as_str());
        let old=heap.get(&key.tools,&property,runtime)?;
        let live=heap.get(tools,&property,runtime)?;
        if !strict_same(&old,&live,values)? { return Ok(false); }
        index+=1;
    }
    Ok(true)
}
fn build_prefix(
    manager: &SharedAppendOnlyContextManager,
    state: &SharedActualAppendOnlyState,
    system_prompt: &[String],
    tools: &[Arc<dyn AgentTool>],
    options: &BuildOptions,
    values: &SessionValues,
) -> Result<PrefixSnapshot, ProviderContextError> {
    // Capture prev before callback-capable metadata. No lock spans a tool Get.
    let previous=manager.lock().unwrap_or_else(|poison|poison.into_inner()).prefix.snapshot.as_ref().map(|snapshot|snapshot.fingerprint.clone());
    let tool_keys=super::typed_tool_keys(tools)?;
    let hit={
        let manager=manager.lock().unwrap_or_else(|poison|poison.into_inner());
        previous.is_some() && manager.prefix.typed_key.as_ref()
            .is_some_and(|key|key.matches(system_prompt,&tool_keys,options))
    };
    if !hit {
        let tools=crate::provider_context::normalize_tools(tools,options)?;
        let fingerprint=prefix_fingerprint(system_prompt,&tools,options);
        let mut manager=manager.lock().unwrap_or_else(|poison|poison.into_inner());
        manager.prefix.typed_key=Some(super::TypedPrefixKey{prompt:system_prompt.to_vec(),tools:tool_keys,options:*options});
        if previous.as_deref()!=Some(fingerprint.as_str()) {
            manager.prefix.original_fingerprint=None;
            manager.prefix.snapshot=Some(StablePrefixSnapshot{system_prompt:system_prompt.to_vec(),tools,fingerprint});
            manager.prefix.version+=1;
        }
    }
    // Publish/project the live snapshot after source's decision. Equal bytes
    // must not resurrect saved prev after reentrant invalidate/rebuild.
    let (snapshot,version)={
        let manager=manager.lock().unwrap_or_else(|poison|poison.into_inner());
        if manager.prefix.original_fingerprint.is_some() {
            return Err(ActualProviderError::Unsupported("reentrant prefix source-domain change").into());
        }
        (manager.prefix.snapshot.clone().ok_or(ProviderContextError::PrefixNotBuilt)?,manager.prefix.version)
    };
    let cached=state.lock().unwrap_or_else(|poison|poison.into_inner()).prefix.clone()
        .filter(|prefix|!prefix.original_tools && prefix.owner_version==version && prefix.fingerprint==snapshot.fingerprint);
    if let Some(cached)=cached { return Ok(cached); }
    let prompt_observation=snapshot.system_prompt.iter().map(|text|WireString::from(text.as_str())).collect::<Vec<_>>();
    let prompt_values=prompt_observation.iter().cloned().map(|text|values.heap().string(text))
        .collect::<Result<Vec<_>,_>>().map_err(ActualProviderError::from)?;
    let mut tool_values=Vec::with_capacity(snapshot.tools.len());
    for tool in &snapshot.tools {
        let source=serde_json::to_value(tool).map_err(|error|ProviderContextError::Schema(error.to_string()))?;
        tool_values.push(values.heap().import_serde(&source).map_err(ActualProviderError::from)?);
    }
    let prefix=PrefixSnapshot{
        fingerprint:snapshot.fingerprint,owner_version:version,original_tools:false,
        prompt:values.heap().array(prompt_values).map_err(ActualProviderError::from)?,
        tools:values.heap().array(tool_values.clone()).map_err(ActualProviderError::from)?,
        prompt_observation,tool_observation:tool_values.into(),
    };
    let mut state=state.lock().unwrap_or_else(|poison|poison.into_inner());
    state.original_key=None;
    state.prefix=Some(prefix.clone());
    Ok(prefix)
}

/// Original prepareProviderCall ordering: sync, normalize/build, provider hook.
/// The caller invokes its existing provider hook only after this returns.
pub fn prepare_append_only_actual(
    manager: &SharedAppendOnlyContextManager,
    system_prompt: &[String],
    tools: &[Arc<dyn AgentTool>],
    messages: &[ValueHandle],
    options: &BuildOptions,
    values: &SessionValues,
) -> Result<ActualProviderContext, ProviderContextError> {
    for message in messages {
        values.check_handle(message).map_err(ActualProviderError::from)?;
    }
    let state = state_for(manager, values)?;
    sync(&state, messages, values)?;
    let prefix = build_prefix(manager, &state, system_prompt, tools, options, values)?;
    context_from_prefix(&state, prefix, values)
}

fn context_from_prefix(
    state: &SharedActualAppendOnlyState, prefix: PrefixSnapshot, values: &SessionValues,
) -> Result<ActualProviderContext, ProviderContextError> {
    let log = entries(&state, values)?;
    let slice = values.heap().get(&log, &"slice".into(), values.runtime()).map_err(ActualProviderError::from)?;
    let message_array = values.heap().call(&slice, &log, &[], values.runtime()).map_err(ActualProviderError::from)?;
    let length = values.heap().array_length(&message_array, values.runtime()).map_err(ActualProviderError::from)?;
    let mut observations = Vec::with_capacity(length as usize);
    for index in 0..length {
        observations.push(
            values
                .heap()
                .get(&message_array, &index.to_string().as_str().into(), values.runtime())
                .map_err(ActualProviderError::from)?,
        );
    }
    let original = values.heap().object().map_err(ActualProviderError::from)?;
    for (key, value) in [("systemPrompt", prefix.prompt), ("messages", message_array), ("tools", prefix.tools)] {
        values
            .heap()
            .define_own_data(&original, key.into(), value, values.runtime())
            .map_err(ActualProviderError::from)?;
    }
    Ok(ActualProviderContext {
        original_context: Some(original),
        system_prompt: prefix.prompt_observation,
        messages: observations,
        tools: Some(prefix.tool_observation),
    })
}

/// Original tool-source path: the same cursor/log/WeakMap is synchronized
/// before current_tools/normalization, exactly as the production preparation.
pub fn prepare_append_only_original_actual(
    manager: &SharedAppendOnlyContextManager, system_prompt: &[String],
    _typed_tools: &[Arc<dyn AgentTool>], messages: &[ValueHandle], options: &BuildOptions,
    values: &SessionValues, source: &dyn crate::ActualToolSource,
) -> Result<ActualProviderContext, ProviderContextError> {
    for message in messages { values.check_handle(message).map_err(ActualProviderError::from)?; }
    let state = state_for(manager, values)?;
    sync(&state, messages, values)?;
    // Source captures prev before current tools and key getters, even on an
    // unbuilt prefix. A reentrant invalidate does not change this saved prev.
    let domain=manager.lock().unwrap_or_else(|poison|poison.into_inner()).prefix.original_fingerprint.clone();
    let previous=state.lock().unwrap_or_else(|poison|poison.into_inner()).prefix.clone()
        .filter(|prefix|prefix.original_tools && domain.as_deref()==Some(prefix.fingerprint.as_str()));
    let original_tools=source.current_tools(values)?.unwrap_or(values.heap().undefined().map_err(ActualProviderError::from)?);
    values.check_handle(&original_tools).map_err(ActualProviderError::from)?;
    let tool_keys=crate::provider_context::graph_tools::prefix_tool_keys(values,&original_tools)
        .map_err(ActualProviderError::from)?;
    let key=state.lock().unwrap_or_else(|poison|poison.into_inner()).original_key.clone();
    if previous.is_some() {
        if let Some(key)=key {
            if original_key_matches(&key,system_prompt,&tool_keys,options,values)? {
                return context_from_prefix(&state,current_prefix(manager,&state)?,values);
            }
        }
    }
    let prompt_observation = system_prompt.iter().map(|text|WireString::from(text.as_str())).collect::<Vec<_>>();
    let prompt_values = prompt_observation.iter().cloned().map(|text|values.heap().string(text))
        .collect::<Result<Vec<_>,_>>().map_err(ActualProviderError::from)?;
    let prompt = values.heap().array(prompt_values).map_err(ActualProviderError::from)?;
    // takeSnapshot reads context.tools again; a miss must not reuse the key's
    // previously observed collection across this callback-capable boundary.
    let original_tools=source.current_tools(values)?.unwrap_or(values.heap().undefined().map_err(ActualProviderError::from)?);
    values.check_handle(&original_tools).map_err(ActualProviderError::from)?;
    let mut tools = crate::provider_context::graph_tools::normalize_original_tools(values,&original_tools,options)
        .map_err(ActualProviderError::from)?;
    if matches!(values.heap().kind(&tools).map_err(ActualProviderError::from)?,ValueKind::Undefined|ValueKind::Null){
        tools=values.heap().array(Vec::new()).map_err(ActualProviderError::from)?;
    }
    let fingerprint=crate::provider_context::graph_tools::fingerprint_original_tools(values,&prompt,&tools,options)
        .map_err(ActualProviderError::from)?;
    // Record only after successful takeSnapshot, also when its bytes equal prev.
    state.lock().unwrap_or_else(|poison|poison.into_inner()).original_key=Some(OriginalPrefixKey{
        prompt:system_prompt.to_vec(),tools:tool_keys,options:*options});
    if previous.as_ref().is_some_and(|prev|prev.fingerprint==fingerprint) {
        return context_from_prefix(&state,current_prefix(manager,&state)?,values);
    }
    let length=values.heap().array_length(&tools,values.runtime()).map_err(ActualProviderError::from)?;
    let mut observed=Vec::new();for index in 0..length{
        observed.push(values.heap().get(&tools,&index.to_string().as_str().into(),values.runtime()).map_err(ActualProviderError::from)?);
    }
    let version={
        let mut manager=manager.lock().unwrap_or_else(|poison|poison.into_inner());
        manager.prefix.snapshot=None;manager.prefix.typed_key=None;
        manager.prefix.original_fingerprint=Some(fingerprint.clone());manager.prefix.version+=1;
        manager.prefix.version
    };
    let prefix=PrefixSnapshot{fingerprint,owner_version:version,original_tools:true,prompt,tools,prompt_observation,tool_observation:observed.into()};
    state.lock().unwrap_or_else(|poison|poison.into_inner()).prefix=Some(prefix.clone());
    context_from_prefix(&state,prefix,values)
}
